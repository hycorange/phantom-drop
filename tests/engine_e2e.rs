//! End-to-end tests: every Phantom Drop transaction is built by the same `ootle-rs` code the CLI
//! uses, then executed by the real Ootle engine (`tari_template_test_tooling`), with real
//! Bulletproofs, balance proofs, condition-tree checks and signature checks.

use std::{
    collections::HashMap,
    num::NonZeroU64,
    sync::{Arc, Weak},
};

use ootle_rs::{
    Address,
    Epoch,
    Network,
    keys::OotleSecretKey,
    provider::{Provider, ProviderResult, WalletProvider, WantInput},
    wallet::OotleWallet,
};
use phantom_drop::{
    EnvelopePlan,
    EnvelopeStatus,
    Funding,
    MasterSeed,
    Path,
    PhantomLink,
    Riddle,
    keys::public_key_bytes,
    ops::{self, build_seal, build_spend, inspect, try_decrypt_substate, wallet_for},
};
use tari_ootle_common_types::engine_types::substate::{Substate, SubstateId};
use tari_ootle_transaction::{Transaction, UnsignedTransaction};
use tari_template_test_tooling::{
    TemplateTest,
    engine_types::virtual_substate::{VirtualSubstate, VirtualSubstateId},
};

const NET: Network = Network::LocalNet;
const TARI: u64 = 1_000_000;
const FEE: u64 = 50_000;

/// An `ootle-rs` provider whose "network" is a snapshot of the in-process engine's state.
struct EngineProvider {
    wallet: OotleWallet,
    substates: HashMap<SubstateId, Substate>,
}

impl EngineProvider {
    fn new(test: &mut TemplateTest, key: OotleSecretKey) -> Self {
        let substates = test
            .get_state_store_mut()
            .iter()
            .map(|(id, s)| (id.clone(), s.clone()))
            .collect();
        Self {
            wallet: wallet_for(key),
            substates,
        }
    }
}

impl Provider for EngineProvider {
    type Client = ();

    fn network(&self) -> Network {
        NET
    }

    fn weak_client(&self) -> Weak<Self::Client> {
        Weak::new()
    }

    fn client_upgrade(&mut self) -> Option<Arc<Self::Client>> {
        None
    }

    fn default_signer_address(&self) -> &Address {
        use ootle_rs::wallet::NetworkWallet;
        self.wallet.default_address()
    }

    async fn resolve_input_want_list(
        &self,
        transaction: UnsignedTransaction,
        _want_list: &std::collections::HashSet<WantInput>,
    ) -> ProviderResult<UnsignedTransaction> {
        // The harness declares every substate as an input itself.
        Ok(transaction)
    }

    async fn fetch_substates<I: IntoIterator<Item = SubstateId> + Send>(
        &self,
        ids: I,
    ) -> ProviderResult<HashMap<SubstateId, Substate>> {
        Ok(ids
            .into_iter()
            .filter_map(|id| self.substates.get(&id).map(|s| (id, s.clone())))
            .collect())
    }
}

impl WalletProvider for EngineProvider {
    type Wallet = OotleWallet;

    fn wallet(&self) -> &OotleWallet {
        &self.wallet
    }

    fn wallet_mut(&mut self) -> &mut OotleWallet {
        &mut self.wallet
    }
}

fn harness() -> TemplateTest {
    let mut test = TemplateTest::new_builtin_only();
    test.enable_fees();
    set_epoch(&mut test, 10);
    test
}

fn set_epoch(test: &mut TemplateTest, epoch: u64) {
    test.set_virtual_substate(VirtualSubstateId::CurrentEpoch, VirtualSubstate::CurrentEpoch(epoch));
}

fn max_epoch(test: &TemplateTest) -> Epoch {
    Epoch(test.current_epoch().as_u64() + 10)
}

fn submit_ok(test: &mut TemplateTest, tx: Transaction) {
    assert!(tx.verify_all_signatures(), "every signature on the transaction must verify");
    test.execute_expect_success(tx, vec![]);
}

fn submit_rejected(test: &mut TemplateTest, tx: Transaction) -> String {
    assert!(tx.verify_all_signatures(), "the transaction is well-signed; rejection must come from the engine");
    let result = test.try_execute(tx, vec![]).expect("transaction processor runs");
    let reason = result
        .finalize
        .result
        .any_reject()
        .map(|r| format!("{r:?}"))
        .expect("transaction must be rejected");
    eprintln!("REJECTED: {reason}");
    reason
}

/// Value of every unspent TARI UTXO addressed to `view_secret`.
fn received(test: &mut TemplateTest, key: &OotleSecretKey) -> Vec<(u64, Option<String>)> {
    test.get_state_store_mut()
        .iter()
        .filter_map(|(id, s)| try_decrypt_substate(id, s, key.view_only_secret()))
        .map(|(_, v, m)| (v, m))
        .collect()
}

struct Sealed {
    master: MasterSeed,
    links: Vec<PhantomLink>,
    plans: Vec<EnvelopePlan>,
}

fn plan(master: &MasterSeed, index: u64, amount: u64, expiry: u64, riddle: Option<Riddle>) -> EnvelopePlan {
    EnvelopePlan::new(
        NET,
        master,
        index,
        NonZeroU64::new(amount).unwrap(),
        expiry,
        "Happy Halloween from the crypt",
        riddle,
    )
    .unwrap()
}

async fn seal_from_faucet(test: &mut TemplateTest, plans: Vec<EnvelopePlan>, master: MasterSeed) -> Sealed {
    let sender = master.wallet_key(NET);
    let provider = EngineProvider::new(test, sender.clone());
    let batch = build_seal(&provider, &sender, Funding::Faucet, &plans, FEE, max_epoch(test))
        .await
        .expect("seal builds");
    submit_ok(test, batch.transaction);
    Sealed {
        master,
        links: batch.links,
        plans,
    }
}

async fn open(test: &mut TemplateTest, link: &PhantomLink, answer: Option<&str>, to: &Address) -> anyhow::Result<Transaction> {
    let ghost = link.seed.ghost_key(link.network);
    let provider = EngineProvider::new(test, ghost);
    let claim_secret = link.seed.claim_secret(answer);
    let (tx, _payout) = build_spend(
        &provider,
        link,
        Path::Claim,
        &claim_secret,
        public_key_bytes(&claim_secret),
        to,
        FEE,
        max_epoch(test),
    )
    .await?;
    Ok(tx)
}

async fn reclaim(test: &mut TemplateTest, sealed: &Sealed, i: usize) -> anyhow::Result<Transaction> {
    let link = &sealed.links[i];
    let plan = &sealed.plans[i];
    let provider = EngineProvider::new(test, link.seed.ghost_key(NET));
    let refund_secret = sealed.master.refund_secret(&link.seed);
    let (tx, _) = build_spend(
        &provider,
        link,
        Path::Refund,
        &refund_secret,
        plan.claim_key,
        &sealed.master.wallet_key(NET).to_address(),
        FEE,
        max_epoch(test),
    )
    .await?;
    Ok(tx)
}

fn riddle() -> Riddle {
    Riddle {
        question: "I have a face but no eyes that see, a candle lit inside of me. What am I?".into(),
        answer: "Jack-o'-lantern".into(),
    }
}

#[tokio::test]
async fn seal_peek_open_and_cannot_open_twice() {
    let mut test = harness();
    let master = MasterSeed::random();
    let plans = vec![plan(&master, 0, 5 * TARI, 100, None), plan(&master, 1, 7 * TARI, 100, None)];
    let sealed = seal_from_faucet(&mut test, plans, master).await;
    assert_eq!(sealed.links.len(), 2);

    // Links survive a round trip through text.
    let link: PhantomLink = sealed.links[1].to_string().parse().unwrap();

    // A link holder can see the amount and the message, nothing more is needed.
    let provider = EngineProvider::new(&mut test, link.seed.ghost_key(NET));
    match inspect(&provider, &link, None).await.unwrap() {
        EnvelopeStatus::Unspent(info) => {
            assert_eq!(info.value, 7 * TARI);
            assert_eq!(info.message, "Happy Halloween from the crypt");
            assert_eq!(info.riddle, None);
            assert_eq!(info.answer_ok, Some(true));
        },
        EnvelopeStatus::Gone => panic!("envelope must exist"),
    }

    // A brand-new recipient with zero funds opens it: the fee is paid from the envelope itself.
    let recipient = OotleSecretKey::random(NET);
    let tx = open(&mut test, &link, None, &recipient.to_address()).await.unwrap();
    submit_ok(&mut test, tx);
    let got = received(&mut test, &recipient);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].0, 7 * TARI - FEE);
    assert_eq!(got[0].1.as_deref(), Some("👻 Phantom Drop opened"));

    // The envelope is gone; opening again is refused before anything is submitted.
    let provider = EngineProvider::new(&mut test, link.seed.ghost_key(NET));
    assert!(matches!(inspect(&provider, &link, None).await.unwrap(), EnvelopeStatus::Gone));
    let again = open(&mut test, &link, None, &recipient.to_address()).await;
    assert!(again.unwrap_err().to_string().contains("already been opened"));

    // The other envelope is untouched.
    let provider = EngineProvider::new(&mut test, sealed.links[0].seed.ghost_key(NET));
    assert!(matches!(
        inspect(&provider, &sealed.links[0], None).await.unwrap(),
        EnvelopeStatus::Unspent(i) if i.value == 5 * TARI
    ));
}

#[tokio::test]
async fn riddle_envelopes_need_the_answer() {
    let mut test = harness();
    let master = MasterSeed::random();
    let sealed = seal_from_faucet(&mut test, vec![plan(&master, 0, 3 * TARI, 100, Some(riddle()))], master).await;
    let link = sealed.links[0].clone();
    assert!(link.riddle);

    let provider = EngineProvider::new(&mut test, link.seed.ghost_key(NET));
    let EnvelopeStatus::Unspent(info) = inspect(&provider, &link, Some("a pumpkin")).await.unwrap() else {
        panic!("envelope must exist");
    };
    assert_eq!(info.riddle.as_deref(), Some(riddle().question.as_str()));
    assert_eq!(info.answer_ok, Some(false));
    let EnvelopeStatus::Unspent(info) = inspect(&provider, &link, Some("  JACK-O'-LANTERN ")).await.unwrap() else {
        panic!("envelope must exist");
    };
    assert_eq!(info.answer_ok, Some(true));

    let thief = OotleSecretKey::random(NET);
    // The client refuses a wrong answer locally...
    let err = open(&mut test, &link, Some("pumpkin"), &thief.to_address()).await.unwrap_err();
    assert!(err.to_string().contains("wrong riddle answer"), "{err}");
    // ...and so does the engine if someone forces it: claim with the no-riddle key but the real tree.
    let wrong_key = link.seed.claim_secret(None);
    let right_key = public_key_bytes(&link.seed.claim_secret(Some("jack-o'-lantern")));
    let provider = EngineProvider::new(&mut test, link.seed.ghost_key(NET));
    let (forged, _) = build_spend(&provider, &link, Path::Claim, &wrong_key, right_key, &thief.to_address(), FEE, max_epoch(&test))
        .await
        .unwrap();
    let reason = submit_rejected(&mut test, forged);
    assert!(reason.to_lowercase().contains("access") || reason.to_lowercase().contains("condition") || reason.to_lowercase().contains("auth"), "{reason}");

    let solver = OotleSecretKey::random(NET);
    let tx = open(&mut test, &link, Some("Jack-O'-Lantern"), &solver.to_address()).await.unwrap();
    submit_ok(&mut test, tx);
    assert_eq!(received(&mut test, &solver)[0].0, 3 * TARI - FEE);
}

#[tokio::test]
async fn expired_links_die_and_only_the_sender_can_reclaim() {
    let mut test = harness();
    let master = MasterSeed::random();
    let sealed = seal_from_faucet(&mut test, vec![plan(&master, 0, 4 * TARI, 20, None)], master).await;
    let link = sealed.links[0].clone();
    let sender = sealed.master.wallet_key(NET);

    // Before expiry the hidden refund branch cannot be used, even by the sender.
    let early = reclaim(&mut test, &sealed, 0).await.unwrap();
    let reason = submit_rejected(&mut test, early);
    assert!(reason.contains("Epoch") || reason.to_lowercase().contains("epoch") || reason.to_lowercase().contains("condition"), "{reason}");

    // After expiry the link is dead: claiming fails in the engine.
    set_epoch(&mut test, 25);
    let late_claimant = OotleSecretKey::random(NET);
    let late = open(&mut test, &link, None, &late_claimant.to_address()).await.unwrap();
    let reason = submit_rejected(&mut test, late);
    assert!(!reason.is_empty());

    // A link holder cannot take the refund branch either: they lack the one-time refund key.
    let provider = EngineProvider::new(&mut test, link.seed.ghost_key(NET));
    let forger = link.seed.claim_secret(None);
    let (forged, _) = build_spend(
        &provider,
        &link,
        Path::Refund,
        &forger,
        sealed.plans[0].claim_key,
        &late_claimant.to_address(),
        FEE,
        max_epoch(&test),
    )
    .await
    .unwrap();
    submit_rejected(&mut test, forged);

    // The sender can, re-deriving everything from their master seed.
    let before = received(&mut test, &sender).iter().map(|r| r.0).sum::<u64>();
    let tx = reclaim(&mut test, &sealed, 0).await.unwrap();
    submit_ok(&mut test, tx);
    let after = received(&mut test, &sender).iter().map(|r| r.0).sum::<u64>();
    assert_eq!(after - before, 4 * TARI - FEE);
}

#[tokio::test]
async fn seal_from_private_utxos_and_on_chain_privacy() {
    let mut test = harness();
    let master = MasterSeed::random();
    let sender = master.wallet_key(NET);

    // Step 0: give the sender a private stealth UTXO (envelope to self, opened by self).
    let bootstrap = seal_from_faucet(&mut test, vec![plan(&master, 99, 50 * TARI, 1000, None)], master.clone()).await;
    let tx = open(&mut test, &bootstrap.links[0], None, &sender.to_address()).await.unwrap();
    submit_ok(&mut test, tx);
    let provider = EngineProvider::new(&mut test, sender.clone());
    let funding: Vec<_> = test
        .get_state_store_mut()
        .iter()
        .filter_map(|(id, s)| try_decrypt_substate(id, s, sender.view_only_secret()))
        .map(|(c, _, _)| c)
        .collect();
    assert_eq!(funding.len(), 1);

    // Step 1: seal two envelopes using only stealth inputs: no account, no revealed input.
    let plans = vec![plan(&master, 0, 10 * TARI, 500, None), plan(&master, 1, 12 * TARI, 500, Some(riddle()))];
    let batch = build_seal(&provider, &sender, Funding::Utxos(funding.clone()), &plans, FEE, max_epoch(&test))
        .await
        .unwrap();
    let seal_bytes = serde_json::to_string(&batch.transaction).unwrap();
    let sender_account_hex = hex::encode(sender.to_address().account_public_key().as_ref());
    assert!(
        !seal_bytes.contains(&sender_account_hex),
        "a UTXO-funded seal must not carry the sender's account key"
    );
    submit_ok(&mut test, batch.transaction);

    // Change came back privately.
    let change: u64 = received(&mut test, &sender).iter().map(|r| r.0).sum();
    assert_eq!(change, 50 * TARI - FEE - 10 * TARI - 12 * TARI - FEE);

    // On-chain, an envelope UTXO carries no key of the ghost, claimant, refunder or sender: only a root.
    let link = &batch.links[0];
    let id = SubstateId::from(ootle_rs::template_types::UtxoAddress::new(
        ootle_rs::template_types::constants::TARI_TOKEN,
        link.commitment.into(),
    ));
    let on_chain = serde_json::to_string(test.get_state_store_mut().iter().find(|(i, _)| **i == id).unwrap().1).unwrap();
    assert!(on_chain.contains("Script"), "sanity: {on_chain:.300}");
    eprintln!("ON-CHAIN ENVELOPE UTXO: {on_chain}");
    let ghost = link.seed.ghost_key(NET).to_address();
    for (what, key) in [
        ("ghost account", hex::encode(ghost.account_public_key().as_ref())),
        ("ghost view", hex::encode(ghost.view_only_key().as_ref())),
        ("claim", hex::encode(plans[0].claim_key.as_ref())),
        ("refund", hex::encode(link.refund_key.as_ref())),
        ("sender", sender_account_hex.clone()),
    ] {
        assert!(!on_chain.contains(&key), "{what} key leaked into the UTXO substate");
    }

    // Opening reveals only the claim leaf: the refund key never appears in the claim transaction.
    let recipient = OotleSecretKey::random(NET);
    let claim = open(&mut test, link, None, &recipient.to_address()).await.unwrap();
    let claim_dbg = serde_json::to_string(&claim).unwrap();
    // Positive control: the revealed claim leaf's one-time key *is* visible, so the searches below are meaningful.
    assert!(claim_dbg.contains(&hex::encode(plans[0].claim_key.as_ref())), "debug output must expose key hex");
    assert!(!claim_dbg.contains(&hex::encode(link.refund_key.as_ref())), "refund branch must stay hidden");
    assert!(!claim_dbg.contains(&sender_account_hex));
    assert!(!claim_dbg.contains(&hex::encode(recipient.to_address().account_public_key().as_ref())));
    submit_ok(&mut test, claim);
    assert_eq!(received(&mut test, &recipient)[0].0, 10 * TARI - FEE);
    let _ = ops::Path::Claim;
}
