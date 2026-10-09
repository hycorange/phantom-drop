//! End-to-end tests: every Phantom Drop transaction is built by the same `ootle-rs` code the CLI
//! uses, then executed by the real Ootle engine (see `phantom_drop::sim`), with real Bulletproofs,
//! balance proofs, condition-tree checks, timelocks, access rules, signatures and fees.

use std::num::NonZeroU64;

use ootle_rs::{
    Address, Network,
    keys::OotleSecretKey,
    template_types::{UtxoAddress, constants::TARI_TOKEN},
};
use phantom_drop::{
    EnvelopePlan, EnvelopeStatus, Funding, MasterSeed, Path, PhantomLink, Riddle,
    keys::public_key_bytes,
    ops::{build_seal, build_spend, inspect, try_decrypt_substate},
    sim::Sim,
};
use tari_ootle_common_types::engine_types::substate::SubstateId;
use tari_ootle_transaction::Transaction;

const NET: Network = Network::LocalNet;
const TARI: u64 = 1_000_000;
const FEE: u64 = 50_000;

struct Sealed {
    master: MasterSeed,
    links: Vec<PhantomLink>,
    plans: Vec<EnvelopePlan>,
}

fn plan(
    master: &MasterSeed,
    index: u64,
    amount: u64,
    expiry: u64,
    riddle: Option<Riddle>,
) -> EnvelopePlan {
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

fn riddle() -> Riddle {
    Riddle {
        question: "I have a face but no eyes that see, a candle lit inside of me. What am I?"
            .into(),
        answer: "Jack-o'-lantern".into(),
    }
}

async fn seal_from_faucet(sim: &mut Sim, plans: Vec<EnvelopePlan>, master: MasterSeed) -> Sealed {
    let sender = master.wallet_key(NET);
    let provider = sim.provider(sender.clone());
    let batch = build_seal(
        &provider,
        &sender,
        Funding::Faucet,
        &plans,
        FEE,
        sim.max_epoch(),
    )
    .await
    .expect("seal builds");
    sim.submit(batch.transaction).expect("seal commits");
    Sealed {
        master,
        links: batch.links,
        plans,
    }
}

async fn open(
    sim: &mut Sim,
    link: &PhantomLink,
    answer: Option<&str>,
    to: &Address,
) -> anyhow::Result<Transaction> {
    let provider = sim.provider(link.seed.ghost_key(link.network));
    let claim_secret = link.seed.claim_secret(answer);
    let (tx, _) = build_spend(
        &provider,
        link,
        Path::Claim,
        &claim_secret,
        public_key_bytes(&claim_secret),
        to,
        FEE,
        sim.max_epoch(),
    )
    .await?;
    Ok(tx)
}

async fn reclaim(sim: &mut Sim, sealed: &Sealed, i: usize) -> anyhow::Result<Transaction> {
    let link = &sealed.links[i];
    let provider = sim.provider(link.seed.ghost_key(NET));
    let (tx, _) = build_spend(
        &provider,
        link,
        Path::Refund,
        &sealed.master.refund_secret(&link.seed),
        sealed.plans[i].claim_key,
        &sealed.master.wallet_key(NET).to_address(),
        FEE,
        sim.max_epoch(),
    )
    .await?;
    Ok(tx)
}

#[tokio::test]
async fn seal_peek_open_and_cannot_open_twice() {
    let mut sim = Sim::new();
    let master = MasterSeed::random();
    let plans = vec![
        plan(&master, 0, 5 * TARI, 100, None),
        plan(&master, 1, 7 * TARI, 100, None),
    ];
    let sealed = seal_from_faucet(&mut sim, plans, master).await;
    assert_eq!(sealed.links.len(), 2);

    // Links survive a round trip through text.
    let link: PhantomLink = sealed.links[1].to_string().parse().unwrap();

    // A link holder can see the amount and the message.
    let provider = sim.provider(link.seed.ghost_key(NET));
    match inspect(&provider, &link, None).await.unwrap() {
        EnvelopeStatus::Unspent(info) => {
            assert_eq!(info.value, 7 * TARI);
            assert_eq!(info.message, "Happy Halloween from the crypt");
            assert_eq!(info.riddle, None);
            assert_eq!(info.answer_ok, Some(true));
        }
        EnvelopeStatus::Gone => panic!("envelope must exist"),
    }

    // A brand-new recipient with zero funds opens it: the fee is paid from the envelope itself.
    let recipient = OotleSecretKey::random(NET);
    let tx = open(&mut sim, &link, None, &recipient.to_address())
        .await
        .unwrap();
    sim.submit(tx).unwrap();
    let got = sim.received(&recipient);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].0, 7 * TARI - FEE);
    assert_eq!(got[0].1.as_deref(), Some("👻 Phantom Drop opened"));

    // The envelope is gone; opening again is refused before anything is submitted.
    let provider = sim.provider(link.seed.ghost_key(NET));
    assert!(matches!(
        inspect(&provider, &link, None).await.unwrap(),
        EnvelopeStatus::Gone
    ));
    let again = open(&mut sim, &link, None, &recipient.to_address()).await;
    assert!(
        again
            .unwrap_err()
            .to_string()
            .contains("already been opened")
    );

    // The other envelope is untouched.
    let provider = sim.provider(sealed.links[0].seed.ghost_key(NET));
    assert!(matches!(
        inspect(&provider, &sealed.links[0], None).await.unwrap(),
        EnvelopeStatus::Unspent(i) if i.value == 5 * TARI
    ));
}

#[tokio::test]
async fn replaying_a_claim_is_rejected_by_the_engine() {
    let mut sim = Sim::new();
    let master = MasterSeed::random();
    let sealed = seal_from_faucet(
        &mut sim,
        vec![plan(&master, 0, 2 * TARI, 100, None)],
        master,
    )
    .await;
    let a = OotleSecretKey::random(NET);
    let b = OotleSecretKey::random(NET);
    // Two racing claims built against the same state: only the first can land.
    let first = open(&mut sim, &sealed.links[0], None, &a.to_address())
        .await
        .unwrap();
    let second = open(&mut sim, &sealed.links[0], None, &b.to_address())
        .await
        .unwrap();
    sim.submit(first).unwrap();
    let reason = sim.submit(second).unwrap_err();
    eprintln!("second claim rejected: {reason}");
    assert!(sim.received(&b).is_empty());
}

#[tokio::test]
async fn riddle_envelopes_need_the_answer() {
    let mut sim = Sim::new();
    let master = MasterSeed::random();
    let sealed = seal_from_faucet(
        &mut sim,
        vec![plan(&master, 0, 3 * TARI, 100, Some(riddle()))],
        master,
    )
    .await;
    let link = sealed.links[0].clone();
    assert!(link.riddle);

    let provider = sim.provider(link.seed.ghost_key(NET));
    let EnvelopeStatus::Unspent(info) = inspect(&provider, &link, Some("a pumpkin")).await.unwrap()
    else {
        panic!("envelope must exist");
    };
    assert_eq!(info.riddle.as_deref(), Some(riddle().question.as_str()));
    assert_eq!(info.answer_ok, Some(false));
    let EnvelopeStatus::Unspent(info) = inspect(&provider, &link, Some("  JACK-O'-LANTERN "))
        .await
        .unwrap()
    else {
        panic!("envelope must exist");
    };
    assert_eq!(info.answer_ok, Some(true));

    let thief = OotleSecretKey::random(NET);
    // The client refuses a wrong answer locally...
    let err = open(&mut sim, &link, Some("pumpkin"), &thief.to_address())
        .await
        .unwrap_err();
    assert!(err.to_string().contains("wrong riddle answer"), "{err}");
    // ...and the engine refuses a forced claim: correct tree, but signed by a key that is not the leaf key.
    let wrong_key = link.seed.claim_secret(None);
    let right_key = public_key_bytes(&link.seed.claim_secret(Some("jack-o'-lantern")));
    let provider = sim.provider(link.seed.ghost_key(NET));
    let (forged, _) = build_spend(
        &provider,
        &link,
        Path::Claim,
        &wrong_key,
        right_key,
        &thief.to_address(),
        FEE,
        sim.max_epoch(),
    )
    .await
    .unwrap();
    let reason = sim.submit(forged).unwrap_err();
    assert!(reason.contains("AccessDenied"), "{reason}");

    let solver = OotleSecretKey::random(NET);
    let tx = open(
        &mut sim,
        &link,
        Some("Jack-O'-Lantern"),
        &solver.to_address(),
    )
    .await
    .unwrap();
    sim.submit(tx).unwrap();
    assert_eq!(sim.received(&solver)[0].0, 3 * TARI - FEE);
}

#[tokio::test]
async fn expired_links_die_and_only_the_sender_can_reclaim() {
    let mut sim = Sim::new();
    let master = MasterSeed::random();
    let sealed =
        seal_from_faucet(&mut sim, vec![plan(&master, 0, 4 * TARI, 20, None)], master).await;
    let link = sealed.links[0].clone();
    let sender = sealed.master.wallet_key(NET);

    // Before expiry the hidden refund branch cannot be used, even by the sender.
    let early = reclaim(&mut sim, &sealed, 0).await.unwrap();
    let reason = sim.submit(early).unwrap_err();
    assert!(reason.contains("AfterEpoch(20)"), "{reason}");

    // After expiry the link is dead: claiming fails in the engine.
    sim.set_epoch(25);
    let late_claimant = OotleSecretKey::random(NET);
    let late = open(&mut sim, &link, None, &late_claimant.to_address())
        .await
        .unwrap();
    let reason = sim.submit(late).unwrap_err();
    assert!(reason.contains("BeforeEpoch(20)"), "{reason}");

    // A link holder cannot take the refund branch either: they lack the one-time refund key.
    let provider = sim.provider(link.seed.ghost_key(NET));
    let (forged, _) = build_spend(
        &provider,
        &link,
        Path::Refund,
        &link.seed.claim_secret(None),
        sealed.plans[0].claim_key,
        &late_claimant.to_address(),
        FEE,
        sim.max_epoch(),
    )
    .await
    .unwrap();
    let reason = sim.submit(forged).unwrap_err();
    assert!(reason.contains("AccessDenied"), "{reason}");

    // The sender can, re-deriving everything from their master seed.
    let before: u64 = sim.received(&sender).iter().map(|r| r.0).sum();
    let tx = reclaim(&mut sim, &sealed, 0).await.unwrap();
    sim.submit(tx).unwrap();
    let after: u64 = sim.received(&sender).iter().map(|r| r.0).sum();
    assert_eq!(after - before, 4 * TARI - FEE);
}

#[tokio::test]
async fn seal_from_private_utxos_and_on_chain_privacy() {
    let mut sim = Sim::new();
    let master = MasterSeed::random();
    let sender = master.wallet_key(NET);

    // Step 0: give the sender a private stealth UTXO (an envelope to self, opened by self).
    let bootstrap = seal_from_faucet(
        &mut sim,
        vec![plan(&master, 99, 50 * TARI, 1000, None)],
        master.clone(),
    )
    .await;
    let tx = open(&mut sim, &bootstrap.links[0], None, &sender.to_address())
        .await
        .unwrap();
    sim.submit(tx).unwrap();
    let funding: Vec<_> = sim
        .engine
        .get_state_store_mut()
        .iter()
        .filter_map(|(id, s)| try_decrypt_substate(id, s, sender.view_only_secret()))
        .map(|(c, _, _)| c)
        .collect();
    assert_eq!(funding.len(), 1);

    // Step 1: seal two envelopes using only stealth inputs: no account, no revealed input.
    let provider = sim.provider(sender.clone());
    let plans = vec![
        plan(&master, 0, 10 * TARI, 500, None),
        plan(&master, 1, 12 * TARI, 500, Some(riddle())),
    ];
    let batch = build_seal(
        &provider,
        &sender,
        Funding::Utxos(funding),
        &plans,
        FEE,
        sim.max_epoch(),
    )
    .await
    .unwrap();
    let seal_json = serde_json::to_string(&batch.transaction).unwrap();
    let sender_account_hex = hex::encode(sender.to_address().account_public_key().as_ref());
    assert!(
        !seal_json.contains(&sender_account_hex),
        "a UTXO-funded seal must not carry the sender's account key"
    );
    sim.submit(batch.transaction).unwrap();

    // Change came back privately.
    let change: u64 = sim.received(&sender).iter().map(|r| r.0).sum();
    assert_eq!(change, 50 * TARI - FEE - 10 * TARI - 12 * TARI - FEE);

    // On-chain, an envelope UTXO carries no key of the ghost, claimant, refunder or sender: only a root.
    let link = &batch.links[0];
    let id = SubstateId::from(UtxoAddress::new(TARI_TOKEN, link.commitment.into()));
    let on_chain = sim.substate_json(&id).unwrap();
    assert!(on_chain.contains("\"Script\""), "{on_chain}");
    let ghost = link.seed.ghost_key(NET).to_address();
    for (what, key) in [
        (
            "ghost account",
            hex::encode(ghost.account_public_key().as_ref()),
        ),
        ("ghost view", hex::encode(ghost.view_only_key().as_ref())),
        ("claim", hex::encode(plans[0].claim_key.as_ref())),
        ("refund", hex::encode(link.refund_key.as_ref())),
        ("sender", sender_account_hex.clone()),
    ] {
        assert!(
            !on_chain.contains(&key),
            "{what} key leaked into the UTXO substate"
        );
    }

    // Opening reveals only the claim leaf: the refund key never appears in the claim transaction.
    let recipient = OotleSecretKey::random(NET);
    let claim = open(&mut sim, link, None, &recipient.to_address())
        .await
        .unwrap();
    let claim_json = serde_json::to_string(&claim).unwrap();
    // Positive control: the revealed claim leaf's one-time key *is* in the transaction.
    assert!(claim_json.contains(&hex::encode(plans[0].claim_key.as_ref())));
    assert!(
        !claim_json.contains(&hex::encode(link.refund_key.as_ref())),
        "refund branch must stay hidden"
    );
    assert!(!claim_json.contains(&sender_account_hex));
    assert!(!claim_json.contains(&hex::encode(
        recipient.to_address().account_public_key().as_ref()
    )));
    sim.submit(claim).unwrap();
    assert_eq!(sim.received(&recipient)[0].0, 10 * TARI - FEE);
}
