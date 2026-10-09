//! Building the three Phantom Drop transactions — seal, open (claim) and reclaim (refund) — plus
//! read-only inspection. Everything here is generic over an `ootle-rs` provider, so the exact same
//! code runs against a live indexer (the CLI) and against the in-process Ootle engine (the tests).

use std::{collections::HashSet, num::NonZeroU64};

use anyhow::{Context, anyhow, bail, ensure};
use async_trait::async_trait;
use ootle_byte_type::FromByteType;
use ootle_rs::{
    Address, Epoch, ToAccountAddress, TransactionRequest,
    builtin_templates::{UnsignedTransactionBuilder, faucet::IFaucet},
    crypto::{
        encrypted_data::unblind_output,
        kdfs::encrypted_data_dh_kdf_aead,
        stealth::{condition_root, script_path_witness},
    },
    key_provider::PrivateKeyProvider,
    keys::OotleSecretKey,
    provider::{Provider, WalletProvider, WantInput},
    signer,
    stealth::{Output, StealthTransfer},
    template_types::{
        Amount, UtxoAddress,
        constants::TARI_TOKEN,
        crypto::{PedersenCommitmentBytes, RistrettoPublicKeyBytes},
        stealth::{SpendAuthorization, StealthInput, StealthTransferStatement},
    },
    transaction::TransactionSealSigner,
    wallet::{NetworkWallet, OotleWallet},
};
use tari_crypto::ristretto::{RistrettoPublicKey, RistrettoSecretKey};
use tari_ootle_common_types::engine_types::substate::SubstateId;
use tari_ootle_transaction::{Transaction, UnsealedTransaction, UnsignedTransaction, args};

use crate::{
    envelope::{EnvelopePlan, envelope_conditions, split_memo},
    keys::public_key_bytes,
    link::PhantomLink,
};

/// Seals a transaction with a single known key and adds no other authorizations. Used for the claim
/// and refund paths, where the leaf's own one-time key is the only identity the transaction carries.
pub struct KeySealer(pub RistrettoSecretKey);

#[async_trait]
impl TransactionSealSigner for KeySealer {
    async fn seal_transaction(
        &self,
        transaction: UnsealedTransaction,
    ) -> signer::Result<Transaction> {
        Ok(transaction.seal(&self.0))
    }
}

pub fn wallet_for(key: OotleSecretKey) -> OotleWallet {
    OotleWallet::from(PrivateKeyProvider::new(key))
}

/// Decrypted, link-holder view of an envelope.
#[derive(Debug, Clone)]
pub struct EnvelopeInfo {
    pub value: u64,
    pub message: String,
    pub riddle: Option<String>,
    /// `Some(true)` if an answer was supplied and it re-derives the committed condition root.
    pub answer_ok: Option<bool>,
    pub expiry_epoch: u64,
}

#[derive(Debug, Clone)]
pub enum EnvelopeStatus {
    /// Still waiting to be opened (or reclaimed).
    Unspent(EnvelopeInfo),
    /// The UTXO no longer exists: it was opened or reclaimed. Which one is not recorded anywhere a
    /// link holder can see — the spending transaction only reveals one leaf.
    Gone,
}

struct FetchedUtxo {
    nonce: RistrettoPublicKey,
    encrypted_data: ootle_rs::template_types::EncryptedData,
    auth: SpendAuthorization,
}

async fn fetch_utxo<P: Provider>(
    provider: &P,
    commitment: PedersenCommitmentBytes,
) -> anyhow::Result<Option<FetchedUtxo>> {
    let id = SubstateId::from(UtxoAddress::new(TARI_TOKEN, commitment.into()));
    let mut found = provider
        .fetch_substates([id.clone()])
        .await
        .map_err(|e| anyhow!("fetching {id}: {e}"))?;
    let Some(substate) = found.remove(&id) else {
        return Ok(None);
    };
    let Some(utxo) = substate.into_substate_value().into_utxo() else {
        bail!("{id} is not a UTXO");
    };
    let Some(output) = utxo.output else {
        return Ok(None);
    };
    let nonce: RistrettoPublicKey = output
        .output
        .public_nonce
        .try_from_byte_type()
        .map_err(|e| anyhow!("malformed nonce on {id}: {e}"))?;
    Ok(Some(FetchedUtxo {
        nonce,
        encrypted_data: output.output.encrypted_data,
        auth: output.auth,
    }))
}

/// Decrypt `(value, memo)` of a UTXO encrypted to `view_secret`.
fn decrypt(
    commitment: &PedersenCommitmentBytes,
    utxo: &FetchedUtxo,
    view_secret: &RistrettoSecretKey,
) -> anyhow::Result<(u64, Option<String>)> {
    let key = encrypted_data_dh_kdf_aead(view_secret, &utxo.nonce);
    let data = unblind_output(commitment, &utxo.encrypted_data, &key, false)
        .map_err(|e| anyhow!("cannot decrypt this UTXO with the given key: {e}"))?;
    let memo = data
        .memo
        .as_ref()
        .and_then(|m| m.as_memo_message().map(str::to_string));
    Ok((data.mask_and_value.value, memo))
}

/// Value of a stealth UTXO owned by `owner` (used to spend the sender's own funding UTXOs).
pub async fn utxo_value<P: Provider>(
    provider: &P,
    owner: &OotleSecretKey,
    commitment: PedersenCommitmentBytes,
) -> anyhow::Result<u64> {
    let utxo = fetch_utxo(provider, commitment)
        .await?
        .ok_or_else(|| anyhow!("UTXO {commitment} is spent or unknown"))?;
    Ok(decrypt(&commitment, &utxo, owner.view_only_secret())?.0)
}

/// What a link holder can learn: amount, message, riddle and expiry — and, if they supply an answer,
/// whether it is right (by recomputing the committed Merkle root).
pub async fn inspect<P: Provider>(
    provider: &P,
    link: &PhantomLink,
    answer: Option<&str>,
) -> anyhow::Result<EnvelopeStatus> {
    let Some(utxo) = fetch_utxo(provider, link.commitment).await? else {
        return Ok(EnvelopeStatus::Gone);
    };
    let ghost = link.seed.ghost_key(link.network);
    let (value, memo) = decrypt(&link.commitment, &utxo, ghost.view_only_secret())?;
    let (message, riddle) = split_memo(memo.as_deref().unwrap_or_default());

    let answer_ok = if link.riddle {
        answer.map(|a| root_matches(link, Some(a), &utxo.auth))
    } else {
        Some(root_matches(link, None, &utxo.auth))
    };
    Ok(EnvelopeStatus::Unspent(EnvelopeInfo {
        value,
        message,
        riddle,
        answer_ok,
        expiry_epoch: link.expiry_epoch,
    }))
}

fn root_matches(link: &PhantomLink, answer: Option<&str>, auth: &SpendAuthorization) -> bool {
    let claim_key = public_key_bytes(&link.seed.claim_secret(answer));
    let conditions = envelope_conditions(claim_key, link.refund_key, link.expiry_epoch);
    let Ok(root) = condition_root(&conditions) else {
        return false;
    };
    matches!(auth, SpendAuthorization::Script(r) if *r == root)
}

/// How the sender pays for the envelopes.
#[derive(Debug, Clone)]
pub enum Funding {
    /// The network's test faucet (localnet / funded testnets): faucet → sender account → envelopes.
    Faucet,
    /// The revealed TARI vault of the sender's account component.
    Account,
    /// The sender's own stealth UTXOs (most private: no account is touched). Change returns as a
    /// fresh stealth UTXO to the sender's address.
    Utxos(Vec<PedersenCommitmentBytes>),
}

pub struct SealedBatch {
    pub transaction: Transaction,
    pub links: Vec<PhantomLink>,
}

/// Builds and signs the transaction that creates one stealth UTXO per plan. `provider`'s wallet
/// must be the sender's wallet.
pub async fn build_seal<P>(
    provider: &P,
    sender: &OotleSecretKey,
    funding: Funding,
    plans: &[EnvelopePlan],
    fee: u64,
    max_epoch: Epoch,
) -> anyhow::Result<SealedBatch>
where
    P: WalletProvider<Wallet = OotleWallet>,
{
    ensure!(!plans.is_empty(), "nothing to seal");
    let network = provider.network();
    let sender_address = sender.to_address();
    let envelopes_total: u64 = plans.iter().map(|p| p.amount.get()).sum();
    let total = envelopes_total + fee;

    let mut transfer = StealthTransfer::new(TARI_TOKEN, provider);
    if let Funding::Utxos(commitments) = &funding {
        let mut available = 0u64;
        for c in commitments {
            available += utxo_value(provider, sender, *c).await?;
            transfer = transfer.spend_stealth_input(sender_address.clone(), *c);
        }
        ensure!(
            available >= total,
            "the selected UTXOs hold {available} µT but {total} µT is needed"
        );
        if let Some(change) = NonZeroU64::new(available - total) {
            transfer =
                transfer.to_stealth_output(Output::new(sender_address.clone(), TARI_TOKEN, change));
        }
    } else {
        transfer = transfer.spend_revealed_input(total);
    }
    for plan in plans {
        transfer = transfer.to_stealth_output(plan.output());
    }
    let (statement, signatures) = transfer.to_revealed_output(fee).prepare().await?;
    let links = match_links(&statement, plans)?;

    let unsigned: UnsignedTransaction = match funding {
        Funding::Faucet => {
            IFaucet::new(provider, max_epoch)
                .take_faucet_funds()
                .into_stealth_transfer(statement)
                .and_pay_fee_from_revealed_output()
                .prepare()
                .await?
        }
        Funding::Account => {
            let account = sender_address.to_account_address();
            let unsigned = Transaction::builder(network, max_epoch)
                .with_auto_fill_inputs()
                .with_fee_instructions_builder(|b| {
                    b.call_method(account, "withdraw", args![TARI_TOKEN, Amount::from(total)])
                        .put_last_instruction_output_on_workspace("phantom_in")
                        .stealth_transfer_with_input_bucket(TARI_TOKEN, statement, "phantom_in")
                        .put_last_instruction_output_on_workspace("phantom_fee")
                        .pay_fee_from_bucket("phantom_fee")
                })
                .build_unsigned();
            let want: HashSet<WantInput> = [
                WantInput::SpecificSubstate {
                    substate_id: account.into(),
                    required: true,
                },
                WantInput::VaultForResource {
                    component_address: account,
                    resource_address: TARI_TOKEN,
                    required: true,
                },
            ]
            .into_iter()
            .collect();
            provider.resolve_input_want_list(unsigned, &want).await?
        }
        Funding::Utxos(commitments) => {
            let mut builder = Transaction::builder(network, max_epoch)
                .with_fee_instructions_builder(|b| {
                    b.stealth_transfer(TARI_TOKEN, statement)
                        .put_last_instruction_output_on_workspace("phantom_fee")
                        .pay_fee_from_bucket("phantom_fee")
                })
                .add_input(TARI_TOKEN);
            for c in commitments {
                builder = builder.add_input(UtxoAddress::new(TARI_TOKEN, c.into()));
            }
            builder.build_unsigned()
        }
    };

    let authorizer = provider.wallet().stealth_authorizer(signatures);
    let transaction = TransactionRequest::default()
        .with_transaction(unsigned)
        .build(&authorizer)
        .await?;
    Ok(SealedBatch { transaction, links })
}

/// Pair each plan with the output that was encrypted to its ghost address. Outputs are matched by
/// trial decryption rather than by position, so a builder that reorders outputs cannot mislabel a link.
fn match_links(
    statement: &StealthTransferStatement,
    plans: &[EnvelopePlan],
) -> anyhow::Result<Vec<PhantomLink>> {
    let outputs = statement.stealth_outputs();
    let mut used = vec![false; outputs.len()];
    let mut links = Vec::with_capacity(plans.len());
    for plan in plans {
        let ghost = plan.seed.ghost_key(plan.network);
        let root = plan.condition_root();
        let mut found = None;
        for (i, out) in outputs.iter().enumerate() {
            if used[i] {
                continue;
            }
            if !matches!(&out.auth, SpendAuthorization::Script(r) if r.into_array() == root) {
                continue;
            }
            let fetched = FetchedUtxo {
                nonce: out
                    .output
                    .sender_public_nonce
                    .try_from_byte_type()
                    .map_err(|e| anyhow!("bad nonce: {e}"))?,
                encrypted_data: out.output.encrypted_data.clone(),
                auth: out.auth.clone(),
            };
            if decrypt(&out.output.commitment, &fetched, ghost.view_only_secret()).is_ok() {
                found = Some(i);
                break;
            }
        }
        let i = found.context("no output in the statement belongs to an envelope plan")?;
        used[i] = true;
        links.push(plan.link(*outputs[i].commitment()));
    }
    Ok(links)
}

/// Which leaf of the envelope tree a spend reveals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Path {
    Claim,
    Refund,
}

/// Builds and signs a transaction that spends the envelope via `path` and sends `value - fee` to
/// `recipient` as a fresh stealth UTXO. The fee comes out of the envelope itself, so the spender
/// needs no funds of their own.
///
/// `provider`'s wallet must be the link's ghost wallet (see [`wallet_for`]); it is only used to
/// decrypt the envelope's mask and value. The transaction is sealed by the leaf key alone.
#[allow(clippy::too_many_arguments)]
pub async fn build_spend<P>(
    provider: &P,
    link: &PhantomLink,
    path: Path,
    leaf_secret: &RistrettoSecretKey,
    claim_key: RistrettoPublicKeyBytes,
    recipient: &Address,
    fee: u64,
    max_epoch: Epoch,
) -> anyhow::Result<(Transaction, u64)>
where
    P: WalletProvider<Wallet = OotleWallet>,
{
    let ghost = link.seed.ghost_key(link.network);
    ensure!(
        provider.wallet().default_address() == &ghost.to_address(),
        "the provider wallet must be the envelope's ghost wallet"
    );
    let utxo = fetch_utxo(provider, link.commitment)
        .await?
        .ok_or_else(|| anyhow!("this envelope has already been opened or reclaimed"))?;
    let (value, _) = decrypt(&link.commitment, &utxo, ghost.view_only_secret())?;
    let payout = value
        .checked_sub(fee)
        .and_then(NonZeroU64::new)
        .ok_or_else(|| {
            anyhow!("the envelope holds {value} µT, not enough to cover a {fee} µT fee")
        })?;

    let conditions = envelope_conditions(claim_key, link.refund_key, link.expiry_epoch);
    let leaf = match path {
        Path::Claim => &conditions[0],
        Path::Refund => &conditions[1],
    };
    let expected_root = condition_root(&conditions)?;
    ensure!(
        matches!(&utxo.auth, SpendAuthorization::Script(r) if *r == expected_root),
        "the keys do not match this envelope's condition tree (wrong riddle answer?)"
    );
    let (witness, _) = script_path_witness(&conditions, leaf)?;
    let leaf_key = public_key_bytes(leaf_secret);

    let (statement, _) = StealthTransfer::new(TARI_TOKEN, provider)
        .spend_stealth_input(
            ghost.to_address(),
            StealthInput::with_witness(link.commitment, witness),
        )
        // The leaf key seals this transaction, so it is the badge that may take the fee bucket.
        .to_revealed_output_for(fee, leaf_key)
        .to_stealth_output(
            Output::new(recipient.clone(), TARI_TOKEN, payout).with_memo_message(match path {
                Path::Claim => "👻 Phantom Drop opened",
                Path::Refund => "👻 Phantom Drop reclaimed",
            }),
        )
        .prepare()
        .await?;

    let unsigned = Transaction::builder(link.network, max_epoch)
        .with_fee_instructions_builder(|b| {
            b.stealth_transfer(TARI_TOKEN, statement)
                .put_last_instruction_output_on_workspace("phantom_fee")
                .pay_fee_from_bucket("phantom_fee")
        })
        .add_input(TARI_TOKEN)
        .add_input(UtxoAddress::new(TARI_TOKEN, link.commitment.into()))
        .build_unsigned();
    let transaction = TransactionRequest::default()
        .with_transaction(unsigned)
        .build(&KeySealer(leaf_secret.clone()))
        .await?;
    Ok((transaction, payout.get()))
}

/// Trial-decrypt a UTXO substate with `view_secret`. Returns `(commitment, value, memo)` when the
/// output is addressed to that key. Used by wallets/tests to find funds they received.
pub fn try_decrypt_substate(
    id: &SubstateId,
    substate: &tari_ootle_common_types::engine_types::substate::Substate,
    view_secret: &RistrettoSecretKey,
) -> Option<(PedersenCommitmentBytes, u64, Option<String>)> {
    let address = id.as_utxo_address()?;
    if *address.resource_address() != TARI_TOKEN {
        return None;
    }
    let commitment = address.id().into_commitment_bytes();
    let utxo = substate.substate_value().as_utxo()?;
    let output = utxo.output.as_ref()?;
    let fetched = FetchedUtxo {
        nonce: output.output.public_nonce.try_from_byte_type().ok()?,
        encrypted_data: output.output.encrypted_data.clone(),
        auth: output.auth.clone(),
    };
    let (value, memo) = decrypt(&commitment, &fetched, view_secret).ok()?;
    Some((commitment, value, memo))
}
