//! A narrated, zero-setup demo of Phantom Drop on the real Ootle engine (in-process).
//!
//! ```bash
//! cargo run --example haunted_demo --features sim
//! ```

use std::num::NonZeroU64;

use ootle_rs::{
    Address, Network,
    keys::OotleSecretKey,
    template_types::{UtxoAddress, constants::TARI_TOKEN},
};
use phantom_drop::{
    EnvelopePlan, EnvelopeStatus, Funding, MasterSeed, Path, PhantomLink, Riddle,
    keys::public_key_bytes,
    ops::{build_seal, build_spend, inspect},
    sim::Sim,
};
use tari_ootle_common_types::engine_types::substate::SubstateId;
use tari_ootle_transaction::Transaction;

const NET: Network = Network::LocalNet;
const TARI: u64 = 1_000_000;
const FEE: u64 = 50_000;

fn tari(micro: u64) -> String {
    format!("{}.{:06} TARI", micro / TARI, micro % TARI)
}

fn act(title: &str) {
    println!("\n\x1b[1;35m━━━ {title} ━━━\x1b[0m");
}

async fn claim(
    sim: &mut Sim,
    link: &PhantomLink,
    answer: Option<&str>,
    to: &Address,
) -> Transaction {
    let provider = sim.provider(link.seed.ghost_key(NET));
    let key = link.seed.claim_secret(answer);
    build_spend(
        &provider,
        link,
        Path::Claim,
        &key,
        public_key_bytes(&key),
        to,
        FEE,
        sim.max_epoch(),
    )
    .await
    .expect("claim builds")
    .0
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut sim = Sim::new();
    println!("👻 PHANTOM DROP — haunted claim links for stealth TARI, on the real Ootle engine");
    println!(
        "   (in-process: real Bulletproofs, balance proofs, condition trees, timelocks, signatures, fees)"
    );

    act("Act I · The crypt keeper seals three envelopes in one transaction");
    let keeper = MasterSeed::random();
    let keeper_wallet = keeper.wallet_key(NET);
    let riddle = Riddle {
        question: "I have a face but no eyes that see, a candle lit inside of me. What am I?"
            .into(),
        answer: "jack-o'-lantern".into(),
    };
    let plans = vec![
        EnvelopePlan::new(
            NET,
            &keeper,
            0,
            NonZeroU64::new(5 * TARI).unwrap(),
            100,
            "🎃 For Alice — welcome to Tari!",
            None,
        )?,
        EnvelopePlan::new(
            NET,
            &keeper,
            1,
            NonZeroU64::new(7 * TARI).unwrap(),
            100,
            "🦇 First to solve it wins",
            Some(riddle.clone()),
        )?,
        EnvelopePlan::new(
            NET,
            &keeper,
            2,
            NonZeroU64::new(3 * TARI).unwrap(),
            20,
            "🕯️ Expires at midnight (epoch 20)",
            None,
        )?,
    ];
    let provider = sim.provider(keeper_wallet.clone());
    let batch = build_seal(
        &provider,
        &keeper_wallet,
        Funding::Faucet,
        &plans,
        FEE,
        sim.max_epoch(),
    )
    .await?;
    sim.submit(batch.transaction).map_err(anyhow::Error::msg)?;
    let links = batch.links;
    for (p, l) in plans.iter().zip(&links) {
        println!(
            "  envelope #{} · {} · expires epoch {}{}",
            p.index,
            tari(p.amount.get()),
            p.expiry_epoch,
            if p.riddle.is_some() {
                " · 🎃 riddle-locked"
            } else {
                ""
            }
        );
        println!("    {l}");
    }

    act("What the chain sees for envelope #0");
    let id = SubstateId::from(UtxoAddress::new(TARI_TOKEN, links[0].commitment.into()));
    println!("  {}", sim.substate_json(&id).unwrap());
    println!("  → a Pedersen commitment, a nonce, ciphertext and a 32-byte Merkle root.");
    println!("    No amount. No owner. No claim key, no refund key, no expiry, no sender.");

    act("Act II · Alice (no wallet, zero balance) opens her link");
    let alice = OotleSecretKey::random(NET);
    let provider = sim.provider(links[0].seed.ghost_key(NET));
    if let EnvelopeStatus::Unspent(info) = inspect(&provider, &links[0], None).await? {
        println!("  peek: {} · \"{}\"", tari(info.value), info.message);
    }
    let tx = claim(&mut sim, &links[0], None, &alice.to_address()).await;
    let signers: Vec<String> = std::iter::once(tx.seal_signature().public_key())
        .chain(tx.signatures().iter().map(|s| s.public_key()))
        .map(|k| format!("{}…", &hex::encode(k.as_ref())[..16]))
        .collect();
    println!("  claim tx signed only by one-time key(s): {signers:?} — not Alice, not the keeper");
    sim.submit(tx).map_err(anyhow::Error::msg)?;
    println!(
        "  ✅ Alice now holds {} as a stealth UTXO (fee was paid from the envelope)",
        tari(sim.received(&alice)[0].0)
    );

    act("Act III · Envelope #1 is posted publicly with its riddle");
    let provider = sim.provider(links[1].seed.ghost_key(NET));
    if let EnvelopeStatus::Unspent(info) = inspect(&provider, &links[1], Some("pumpkin")).await? {
        println!(
            "  peek: {} · riddle: \"{}\"",
            tari(info.value),
            info.riddle.unwrap_or_default()
        );
        println!(
            "  Bob guesses \"pumpkin\" → answer ok? {:?}",
            info.answer_ok
        );
    }
    let mallory = OotleSecretKey::random(NET);
    let right = public_key_bytes(&links[1].seed.claim_secret(Some(&riddle.answer)));
    let (forged, _) = build_spend(
        &provider,
        &links[1],
        Path::Claim,
        &links[1].seed.claim_secret(Some("pumpkin")),
        right,
        &mallory.to_address(),
        FEE,
        sim.max_epoch(),
    )
    .await?;
    println!(
        "  Mallory forces a claim with the wrong key → engine says: {}",
        sim.submit(forged).unwrap_err()
    );
    let carol = OotleSecretKey::random(NET);
    let tx = claim(
        &mut sim,
        &links[1],
        Some("Jack-O'-Lantern"),
        &carol.to_address(),
    )
    .await;
    sim.submit(tx).map_err(anyhow::Error::msg)?;
    println!(
        "  ✅ Carol solved it and holds {}",
        tari(sim.received(&carol)[0].0)
    );

    act("Act IV · Midnight passes; envelope #2's link leaks");
    sim.set_epoch(25);
    let dave = OotleSecretKey::random(NET);
    let tx = claim(&mut sim, &links[2], None, &dave.to_address()).await;
    println!(
        "  Dave tries the leaked link → engine says: {}",
        sim.submit(tx).unwrap_err()
    );
    let provider = sim.provider(links[2].seed.ghost_key(NET));
    let (refund, payout) = build_spend(
        &provider,
        &links[2],
        Path::Refund,
        &keeper.refund_secret(&links[2].seed),
        plans[2].claim_key,
        &keeper_wallet.to_address(),
        FEE,
        sim.max_epoch(),
    )
    .await?;
    sim.submit(refund).map_err(anyhow::Error::msg)?;
    println!(
        "  ✅ the keeper reclaims {} through the hidden refund branch — revealed only now",
        tari(payout)
    );

    act("Act V · Nothing to haunt twice");
    let provider = sim.provider(links[1].seed.ghost_key(NET));
    let key = links[1].seed.claim_secret(Some("jack-o'-lantern"));
    match build_spend(
        &provider,
        &links[1],
        Path::Claim,
        &key,
        public_key_bytes(&key),
        &dave.to_address(),
        FEE,
        sim.max_epoch(),
    )
    .await
    {
        Ok(_) => anyhow::bail!("a spent envelope must not be claimable"),
        Err(e) => println!("  Dave replays Carol's solved link → {e}"),
    }
    let status = inspect(&provider, &links[1], None).await?;
    println!(
        "  envelope #1 status: {}",
        if matches!(status, EnvelopeStatus::Gone) {
            "Gone 👻"
        } else {
            "still unspent?!"
        }
    );
    println!(
        "\n🎃 Every transaction above was built by the same ootle-rs code the `phantom` CLI runs against Esmeralda."
    );
    Ok(())
}
