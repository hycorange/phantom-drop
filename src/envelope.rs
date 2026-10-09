//! Envelope construction: the two-leaf condition tree and the stealth output that carries it.

use std::num::NonZeroU64;

use ootle_rs::{
    Address,
    Network,
    crypto::stealth::condition_root,
    stealth::Output,
    template_types::{
        crypto::{PedersenCommitmentBytes, RistrettoPublicKeyBytes},
        rule,
        stealth::{AtomicCondition, BuiltinPredicate, SpendCondition},
        constants::TARI_TOKEN,
    },
};
use tari_crypto::ristretto::RistrettoSecretKey;

use crate::{
    keys::{EnvelopeSeed, MasterSeed, public_key_bytes},
    link::PhantomLink,
};

/// Max memo payload Ootle accepts for a message memo.
pub const MAX_MEMO_LEN: usize = 253;

/// Claim leaf: a signature from the (one-time) claim key, strictly before the expiry epoch.
pub fn claim_leaf(claim_key: RistrettoPublicKeyBytes, expiry_epoch: u64) -> SpendCondition {
    SpendCondition::all([
        AtomicCondition::AccessRule(rule!(public_key(claim_key))),
        AtomicCondition::Builtin(BuiltinPredicate::BeforeEpoch(expiry_epoch)),
    ])
}

/// Refund leaf: a signature from the sender's one-time refund key, at or after the expiry epoch.
/// It stays hidden behind the Merkle root unless the sender actually reclaims.
pub fn refund_leaf(refund_key: RistrettoPublicKeyBytes, expiry_epoch: u64) -> SpendCondition {
    SpendCondition::all([
        AtomicCondition::Builtin(BuiltinPredicate::AfterEpoch(expiry_epoch)),
        AtomicCondition::AccessRule(rule!(public_key(refund_key))),
    ])
}

/// The committed set, in the canonical order `[claim, refund]`. Order does not change the root (leaf
/// hashes are sorted) but the code always indexes leaves by this order.
pub fn envelope_conditions(
    claim_key: RistrettoPublicKeyBytes,
    refund_key: RistrettoPublicKeyBytes,
    expiry_epoch: u64,
) -> Vec<SpendCondition> {
    vec![claim_leaf(claim_key, expiry_epoch), refund_leaf(refund_key, expiry_epoch)]
}

#[derive(Debug, Clone)]
pub struct Riddle {
    pub question: String,
    pub answer: String,
}

/// Everything the sender knows about one envelope before it is sealed.
#[derive(Clone)]
pub struct EnvelopePlan {
    pub network: Network,
    pub index: u64,
    pub seed: EnvelopeSeed,
    pub amount: NonZeroU64,
    pub expiry_epoch: u64,
    pub claim_key: RistrettoPublicKeyBytes,
    pub refund_secret: RistrettoSecretKey,
    pub refund_key: RistrettoPublicKeyBytes,
    pub riddle: Option<Riddle>,
    pub memo: String,
}

impl EnvelopePlan {
    pub fn new(
        network: Network,
        master: &MasterSeed,
        index: u64,
        amount: NonZeroU64,
        expiry_epoch: u64,
        message: &str,
        riddle: Option<Riddle>,
    ) -> anyhow::Result<Self> {
        let seed = master.envelope_seed(index);
        let claim_secret = seed.claim_secret(riddle.as_ref().map(|r| r.answer.as_str()));
        let refund_secret = master.refund_secret(&seed);
        let memo = compose_memo(message, riddle.as_ref().map(|r| r.question.as_str()));
        anyhow::ensure!(
            memo.len() <= MAX_MEMO_LEN,
            "message + riddle is {} bytes; the encrypted memo holds at most {MAX_MEMO_LEN}",
            memo.len()
        );
        Ok(Self {
            network,
            index,
            seed,
            amount,
            expiry_epoch,
            claim_key: public_key_bytes(&claim_secret),
            refund_key: public_key_bytes(&refund_secret),
            refund_secret,
            riddle,
            memo,
        })
    }

    pub fn ghost_address(&self) -> Address {
        self.seed.ghost_key(self.network).to_address()
    }

    pub fn conditions(&self) -> Vec<SpendCondition> {
        envelope_conditions(self.claim_key, self.refund_key, self.expiry_epoch)
    }

    pub fn condition_root(&self) -> [u8; 32] {
        let root = condition_root(&self.conditions()).expect("two distinct leaves form a valid tree");
        root.into_array()
    }

    /// The stealth output: value encrypted to the ghost address, spendable only via the condition tree.
    pub fn output(&self) -> Output {
        Output::new(self.ghost_address(), TARI_TOKEN, self.amount)
            .with_spend_conditions(self.conditions())
            .with_memo_message(self.memo.clone())
    }

    pub fn link(&self, commitment: PedersenCommitmentBytes) -> PhantomLink {
        PhantomLink {
            network: self.network,
            riddle: self.riddle.is_some(),
            seed: self.seed,
            commitment,
            refund_key: self.refund_key,
            expiry_epoch: self.expiry_epoch,
        }
    }
}

const RIDDLE_MARKER: &str = "\n🎃 riddle: ";

pub fn compose_memo(message: &str, riddle_question: Option<&str>) -> String {
    match riddle_question {
        Some(q) => format!("{message}{RIDDLE_MARKER}{q}"),
        None => message.to_string(),
    }
}

/// Splits a memo into `(message, riddle question)`.
pub fn split_memo(memo: &str) -> (String, Option<String>) {
    match memo.split_once(RIDDLE_MARKER) {
        Some((m, q)) => (m.to_string(), Some(q.to_string())),
        None => (memo.to_string(), None),
    }
}
