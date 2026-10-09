//! # Phantom Drop
//!
//! Haunted claim links for stealth TARI on Tari Ootle. See the README for the design.

pub mod envelope;
pub mod keys;
pub mod link;
pub mod ops;
pub mod scan;
#[cfg(feature = "sim")]
pub mod sim;

pub use envelope::{EnvelopePlan, Riddle};
pub use keys::{EnvelopeSeed, MasterSeed};
pub use link::PhantomLink;
pub use ops::{EnvelopeStatus, Funding, Path};
