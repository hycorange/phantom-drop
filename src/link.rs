//! The Phantom link: everything a recipient needs to find, decrypt and claim one envelope.
//!
//! Layout (v1, 107 bytes before the checksum):
//! `version(1) | network(1) | flags(1) | seed(32) | utxo commitment(32) | refund leaf key(32) | expiry epoch(8, LE)`
//! followed by a 4-byte Blake2b checksum, base58-encoded behind a `phantom:` prefix.
//!
//! The refund key is a one-time key: it is only in the link because the claimant needs the full
//! refund leaf to build the Merkle inclusion proof for the claim leaf. It identifies nobody.

use std::{fmt, str::FromStr};

use ootle_rs::{
    Network,
    template_types::crypto::{PedersenCommitmentBytes, RistrettoPublicKeyBytes},
};

use crate::keys::{EnvelopeSeed, hash32};

pub const LINK_PREFIX: &str = "phantom:";
const VERSION: u8 = 1;
const FLAG_RIDDLE: u8 = 0b0000_0001;
const BODY_LEN: usize = 3 + 32 + 32 + 32 + 8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhantomLink {
    pub network: Network,
    /// When set, the claim key also needs the riddle answer.
    pub riddle: bool,
    pub seed: EnvelopeSeed,
    pub commitment: PedersenCommitmentBytes,
    pub refund_key: RistrettoPublicKeyBytes,
    pub expiry_epoch: u64,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum LinkError {
    #[error("a phantom link starts with `{LINK_PREFIX}`")]
    MissingPrefix,
    #[error("the link is not valid base58")]
    Base58,
    #[error("the link has the wrong length ({0} bytes)")]
    Length(usize),
    #[error("the link checksum does not match (typo or truncated link?)")]
    Checksum,
    #[error("unsupported link version {0}")]
    Version(u8),
    #[error("unknown network byte {0:#x}")]
    Network(u8),
    #[error("the link contains an invalid key or commitment")]
    InvalidBytes,
}

impl PhantomLink {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(BODY_LEN + 4);
        out.push(VERSION);
        out.push(self.network.as_byte());
        out.push(if self.riddle { FLAG_RIDDLE } else { 0 });
        out.extend_from_slice(&self.seed.0);
        out.extend_from_slice(self.commitment.as_ref());
        out.extend_from_slice(self.refund_key.as_ref());
        out.extend_from_slice(&self.expiry_epoch.to_le_bytes());
        let checksum = hash32("link/checksum", &[&out]);
        out.extend_from_slice(&checksum[..4]);
        out
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, LinkError> {
        if bytes.len() != BODY_LEN + 4 {
            return Err(LinkError::Length(bytes.len()));
        }
        let (body, checksum) = bytes.split_at(BODY_LEN);
        if hash32("link/checksum", &[body])[..4] != *checksum {
            return Err(LinkError::Checksum);
        }
        if body[0] != VERSION {
            return Err(LinkError::Version(body[0]));
        }
        let network = Network::try_from(body[1]).map_err(|_| LinkError::Network(body[1]))?;
        let riddle = body[2] & FLAG_RIDDLE != 0;
        let mut seed = [0u8; 32];
        seed.copy_from_slice(&body[3..35]);
        let mut commitment = [0u8; 32];
        commitment.copy_from_slice(&body[35..67]);
        let mut refund = [0u8; 32];
        refund.copy_from_slice(&body[67..99]);
        let mut expiry = [0u8; 8];
        expiry.copy_from_slice(&body[99..107]);
        Ok(Self {
            network,
            riddle,
            seed: EnvelopeSeed(seed),
            commitment: PedersenCommitmentBytes::from(commitment),
            refund_key: RistrettoPublicKeyBytes::from(refund),
            expiry_epoch: u64::from_le_bytes(expiry),
        })
    }
}

impl fmt::Display for PhantomLink {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{LINK_PREFIX}{}", bs58::encode(self.to_bytes()).into_string())
    }
}

impl FromStr for PhantomLink {
    type Err = LinkError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        // Accept a bare link or a web URL carrying it in the fragment, e.g. https://example/#phantom:...
        let s = s.rsplit_once('#').map(|(_, frag)| frag).unwrap_or(s);
        let encoded = s.strip_prefix(LINK_PREFIX).ok_or(LinkError::MissingPrefix)?;
        let bytes = bs58::decode(encoded).into_vec().map_err(|_| LinkError::Base58)?;
        Self::from_bytes(&bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> PhantomLink {
        PhantomLink {
            network: Network::Esmeralda,
            riddle: true,
            seed: EnvelopeSeed([9u8; 32]),
            commitment: PedersenCommitmentBytes::from([3u8; 32]),
            refund_key: RistrettoPublicKeyBytes::from([4u8; 32]),
            expiry_epoch: 4242,
        }
    }

    #[test]
    fn round_trips() {
        let link = sample();
        let s = link.to_string();
        assert!(s.starts_with(LINK_PREFIX));
        assert_eq!(s.parse::<PhantomLink>().unwrap(), link);
        let url = format!("https://phantom.example/open#{s}");
        assert_eq!(url.parse::<PhantomLink>().unwrap(), link);
    }

    #[test]
    fn detects_typos() {
        let s = sample().to_string();
        let mut chars: Vec<char> = s.chars().collect();
        let i = chars.len() - 10;
        chars[i] = if chars[i] == 'A' { 'B' } else { 'A' };
        let typo: String = chars.into_iter().collect();
        assert_eq!(typo.parse::<PhantomLink>().unwrap_err(), LinkError::Checksum);
        assert_eq!("phantom".parse::<PhantomLink>().unwrap_err(), LinkError::MissingPrefix);
    }
}
