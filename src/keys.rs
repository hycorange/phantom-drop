//! Key derivation for Phantom Drop.
//!
//! Every key an envelope needs is derived from a 32-byte *envelope seed* with domain-separated
//! Blake2b-512 hashing, reduced to a Ristretto scalar. Nothing is stored on-chain that links the
//! derived keys to each other or to the sender's wallet.

use argon2::{Algorithm, Argon2, Params, Version};
use blake2::{Blake2b512, Digest};
use ootle_byte_type::ToByteType;
use ootle_rs::{Network, keys::OotleSecretKey};
use ootle_rs::template_types::crypto::RistrettoPublicKeyBytes;
use rand::RngCore;
use tari_crypto::{
    keys::{PublicKey, SecretKey},
    ristretto::{RistrettoPublicKey, RistrettoSecretKey},
};
use zeroize::{Zeroize, Zeroizing};

const DOMAIN: &[u8] = b"phantom-drop/v1/";

/// Hash `parts` under `label` (length-prefixed, so part boundaries are unambiguous) into a uniform scalar.
pub fn hash_to_scalar(label: &str, parts: &[&[u8]]) -> RistrettoSecretKey {
    let mut hasher = Blake2b512::new();
    hasher.update(DOMAIN);
    hasher.update((label.len() as u64).to_le_bytes());
    hasher.update(label.as_bytes());
    for part in parts {
        hasher.update((part.len() as u64).to_le_bytes());
        hasher.update(part);
    }
    let mut wide = hasher.finalize();
    let key = RistrettoSecretKey::from_uniform_bytes(&wide).expect("64 bytes is the uniform scalar length");
    wide.as_mut_slice().zeroize();
    key
}

pub fn hash32(label: &str, parts: &[&[u8]]) -> [u8; 32] {
    let mut hasher = Blake2b512::new();
    hasher.update(DOMAIN);
    hasher.update((label.len() as u64).to_le_bytes());
    hasher.update(label.as_bytes());
    for part in parts {
        hasher.update((part.len() as u64).to_le_bytes());
        hasher.update(part);
    }
    let out = hasher.finalize();
    let mut res = [0u8; 32];
    res.copy_from_slice(&out[..32]);
    res
}

pub fn public_key_bytes(secret: &RistrettoSecretKey) -> RistrettoPublicKeyBytes {
    RistrettoPublicKey::from_secret_key(secret).to_byte_type()
}

/// The sender's master secret. Envelope seeds and refund keys are derived from it, so a sender who
/// keeps only this secret can re-derive and reclaim every expired envelope they ever sealed.
#[derive(Clone)]
pub struct MasterSeed(Zeroizing<[u8; 32]>);

impl MasterSeed {
    pub fn random() -> Self {
        let mut bytes = [0u8; 32];
        rand::rng().fill_bytes(&mut bytes);
        Self(Zeroizing::new(bytes))
    }

    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(Zeroizing::new(bytes))
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// The sender's own spending wallet (funds envelopes, receives refunds).
    pub fn wallet_key(&self, network: Network) -> OotleSecretKey {
        OotleSecretKey::new(
            network,
            hash_to_scalar("wallet/account", &[self.0.as_ref()]),
            hash_to_scalar("wallet/view", &[self.0.as_ref()]),
        )
    }

    /// Deterministic seed of envelope number `index`.
    pub fn envelope_seed(&self, index: u64) -> EnvelopeSeed {
        EnvelopeSeed(hash32("envelope/seed", &[self.0.as_ref(), &index.to_le_bytes()]))
    }

    /// One-time refund key for the envelope with `seed`. Never reused, so a refund reveals nothing
    /// that links it to the sender's other envelopes or wallet.
    pub fn refund_secret(&self, seed: &EnvelopeSeed) -> RistrettoSecretKey {
        hash_to_scalar("envelope/refund", &[self.0.as_ref(), &seed.0])
    }
}

/// The secret carried in a Phantom link. Whoever holds it can find, decrypt and (unless a riddle
/// guards it) claim the envelope.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct EnvelopeSeed(pub [u8; 32]);

impl EnvelopeSeed {
    pub fn random() -> Self {
        let mut bytes = [0u8; 32];
        rand::rng().fill_bytes(&mut bytes);
        Self(bytes)
    }

    /// The ghost wallet: a throw-away Ootle address the envelope UTXO is encrypted to. Its view key
    /// lets a link holder decrypt the amount, mask and memo.
    pub fn ghost_key(&self, network: Network) -> OotleSecretKey {
        OotleSecretKey::new(
            network,
            hash_to_scalar("ghost/account", &[&self.0]),
            hash_to_scalar("ghost/view", &[&self.0]),
        )
    }

    /// The key that satisfies the claim leaf. Without a riddle it follows from the seed alone; with a
    /// riddle it also needs the answer, stretched with Argon2id so guesses are expensive.
    pub fn claim_secret(&self, answer: Option<&str>) -> RistrettoSecretKey {
        match answer {
            None => hash_to_scalar("claim/plain", &[&self.0]),
            Some(answer) => {
                let stretched = stretch_answer(&self.0, answer, &KdfCost::default());
                hash_to_scalar("claim/riddle", &[&self.0, stretched.as_ref()])
            },
        }
    }
}

/// Argon2id cost parameters for riddle answers.
#[derive(Debug, Clone)]
pub struct KdfCost {
    pub memory_kib: u32,
    pub iterations: u32,
}

impl Default for KdfCost {
    fn default() -> Self {
        // 64 MiB, 3 passes: roughly a quarter to half a second per guess on a laptop core.
        Self {
            memory_kib: 64 * 1024,
            iterations: 3,
        }
    }
}

/// Answers are compared case-insensitively with whitespace collapsed, so "Jack O Lantern " and
/// "jack o lantern" open the same envelope.
pub fn normalize_answer(answer: &str) -> String {
    answer
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

pub fn stretch_answer(seed: &[u8; 32], answer: &str, cost: &KdfCost) -> Zeroizing<[u8; 32]> {
    let normalized = Zeroizing::new(normalize_answer(answer));
    let salt = hash32("riddle/salt", &[seed]);
    let params = Params::new(cost.memory_kib, cost.iterations, 1, Some(32)).expect("valid argon2 params");
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut out = Zeroizing::new([0u8; 32]);
    argon
        .hash_password_into(normalized.as_bytes(), &salt, out.as_mut())
        .expect("argon2 output length is valid");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derivations_are_deterministic_and_domain_separated() {
        let master = MasterSeed::from_bytes([7u8; 32]);
        let a = master.envelope_seed(0);
        assert_eq!(a, master.envelope_seed(0));
        assert_ne!(a, master.envelope_seed(1));
        let ghost = a.ghost_key(Network::LocalNet);
        assert_ne!(ghost.account_secret(), ghost.view_only_secret());
        assert_ne!(&a.claim_secret(None), ghost.account_secret());
        assert_ne!(master.refund_secret(&a), a.claim_secret(None));
    }

    #[test]
    fn answers_are_normalized() {
        assert_eq!(normalize_answer("  Jack   O\tLantern "), "jack o lantern");
        let seed = EnvelopeSeed([1u8; 32]);
        let cheap = KdfCost {
            memory_kib: 64,
            iterations: 1,
        };
        assert_eq!(
            stretch_answer(&seed.0, "Pumpkin", &cheap),
            stretch_answer(&seed.0, " pumpkin ", &cheap)
        );
        assert_ne!(
            stretch_answer(&seed.0, "pumpkin", &cheap),
            stretch_answer(&seed.0, "pumpkins", &cheap)
        );
    }
}
