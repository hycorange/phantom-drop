# 👻 Phantom Drop — haunted claim links for stealth TARI on Tari Ootle

[![CI](https://github.com/hycorange/phantom-drop/actions/workflows/ci.yml/badge.svg)](https://github.com/hycorange/phantom-drop/actions/workflows/ci.yml)

Repository: <https://gitlab.com/hycorange/phantom-drop> (primary) · GitHub mirror: <https://github.com/hycorange/phantom-drop>

**Try it in the browser:** <https://hycorange.github.io/phantom-drop/> (step-through of the five acts, what the chain sees vs. what the link holder sees, and the live Esmeralda run).

**Phantom Drop turns an amount of TARI into a secret link.** Whoever holds the link can open it
and receive the funds privately, **even if they have no wallet and zero balance**. If nobody opens it
before midnight (an expiry epoch), the link dies and the sender can quietly take the money back.
Optional **riddle locks** mean the link alone is not enough: you also need the answer.

Built for the Tari *October Build Contest — Spooky Secrets*. It uses only **native Ootle stealth
primitives** (no custom template): stealth outputs, Pedersen commitments + Bulletproofs, encrypted
memos, **MAST spend‑condition trees** with a *hidden* refund branch, epoch timelocks and one‑time keys.

```
sender ──seal──▶  stealth UTXO  (commitment + ciphertext + 32-byte Merkle root)
                     │
       phantom:3d3…  │  (link shared off-chain: DM, QR, paper, a tweet…)
                     ▼
   ┌─────────────── condition tree (only the root is on chain) ───────────────┐
   │ claim leaf : sig(one-time claim key[, from riddle answer]) ∧ epoch < E   │
   │ refund leaf: sig(one-time refund key)                     ∧ epoch ≥ E   │
   └───────────────────────────────────────────────────────────────────────────┘
        open (before E): fee paid out of the envelope itself → recipient's stealth UTXO
        reclaim (after E): only now is the refund leaf revealed → back to sender
```

## Why it's spooky

| What an observer of the chain sees for a sealed envelope | |
|---|---|
| amount | ❌ hidden (Pedersen commitment + range proof) |
| recipient / owner | ❌ none — it is locked by a Merkle root, not a public key |
| claim key, refund key, expiry | ❌ hidden behind the root until a branch is used |
| message / riddle | ❌ encrypted memo, readable only with the link |
| sender | ❌ not linkable when funded from stealth UTXOs (`--fund utxos`) |

* **Zero‑funds claim.** The recipient never needs TARI first: the claim transaction is sealed by the
  envelope's own one‑time key and pays its fee from the envelope value (`5 TARI` in → `4.95 TARI` out).
* **Unclaimable after expiry, reclaimable only by the sender.** Enforced by the engine
  (`BeforeEpoch`/`AfterEpoch` built‑ins), not by the app. The refund branch stays invisible unless used.
* **Riddle locks.** The claim key is derived from `Argon2id(answer)` (64 MiB, 3 passes), so the link can
  be posted publicly ("first to solve it wins") without anyone being able to front‑run with the link alone.
* **Double‑open is impossible.** The UTXO is consumed by the first valid spend; the engine rejects replays.
* **Batching.** Seal up to 64 envelopes in one transaction (one fee, one proof set).
* **Deterministic & recoverable.** Every envelope secret derives from one master seed + index
  (`Blake2b`, domain‑separated), so the sender can recompute all links and refund keys.

## Quick start

Requires Rust (the pinned toolchain in `rust-toolchain.toml` is installed automatically by rustup).

```bash
git clone https://gitlab.com/hycorange/phantom-drop.git && cd phantom-drop
cargo test                                               # unit + 5 end-to-end engine tests
cargo run --example haunted_demo --features sim 2>/dev/null   # narrated 5-act demo (see docs/demo-output.txt)
cargo build --release                                    # CLI: target/release/phantom
```

### The haunted demo (runs fully offline)

`examples/haunted_demo.rs` drives the **real Ootle engine in‑process** (via `tari_template_test_tooling`) with
the exact same transaction‑building code the CLI uses against the testnet: real Bulletproofs, balance proofs,
condition trees, timelock checks, signature checks and fees.

1. The crypt keeper seals three envelopes in one transaction (plain, riddle‑locked, short expiry).
2. What the chain sees: commitment, nonce, ciphertext and a Merkle root — nothing else.
3. Alice (no wallet, no balance) opens her link and ends up with 4.95 TARI.
4. A riddle link is posted publicly: Bob guesses wrong, Mallory forces a wrong key → `AccessDenied`;
   Carol solves it and gets paid.
5. Midnight passes: a leaked link is rejected with `BeforeEpoch(20)`; the keeper reclaims through the
   hidden refund branch. Finally, replaying an opened link fails: “already opened or reclaimed”.

### Tests

`tests/engine_e2e.rs` (all executed against the real engine):

| test | proves |
|---|---|
| `seal_peek_open_and_cannot_open_twice` | seal → peek → open with zero balance; second open fails |
| `replaying_a_claim_is_rejected_by_the_engine` | an identical claim tx is rejected the second time |
| `riddle_envelopes_need_the_answer` | wrong answer → `AccessDenied`; right answer works (normalised) |
| `expired_links_die_and_only_the_sender_can_reclaim` | claim after expiry → `BeforeEpoch`; early refund → `AfterEpoch`; strangers → `AccessDenied` |
| `seal_from_private_utxos_and_on_chain_privacy` | funding from stealth UTXOs; on‑chain JSON contains no amount/keys/expiry (with a positive control) |

Plus unit tests for key derivation/domain separation, answer normalisation and link encoding/checksums.

CI runs on GitHub Actions ([`.github/workflows/ci.yml`](.github/workflows/ci.yml)): `cargo fmt --check`, `cargo test`
and the haunted demo, on every push to `main`. Runs: <https://github.com/hycorange/phantom-drop/actions/workflows/ci.yml>.
GitLab stays the primary repository; its pipeline ([`.gitlab-ci.yml`](.gitlab-ci.yml)) is kept but doesn't run there
because of account limits. Latest local run: [`docs/test-output.txt`](docs/test-output.txt) (9 tests, all passing).

## CLI on the Esmeralda testnet

```bash
phantom init                          # creates ~/.phantom/esmeralda/master.phantom-secret
phantom address                       # your sender address (fund it with testnet TARI)
phantom status                        # indexer + current epoch
phantom balance                       # scan the chain for your stealth UTXOs

# seal 3 links of 2 TARI each, valid for ~200 epochs
phantom seal --amount 2 --count 3 --message "👻 Happy Halloween!"
# riddle-locked
phantom seal --amount 5 --riddle "What has a face but no eyes?" --answer "jack-o'-lantern"
# --fund utxos (default, private) | account | faucet ; --dry-run builds+signs without submitting

phantom peek phantom:3d3…             # amount, message, riddle, expiry, still unopened?
phantom open phantom:3d3… --to otl_esm_…          # claim into an existing address
phantom open phantom:3d3… --new-wallet me.key     # …or into a brand-new key file
phantom open phantom:3d3… --answer "jack-o'-lantern" --to otl_esm_…
phantom list                          # your sealed envelopes and their live status
phantom reclaim                       # reclaim every expired, unopened envelope
```

Global flags: `--network esmeralda|localnet`, `--indexer <url>`, `--home <dir>` (or `PHANTOM_HOME`).
Links are also accepted as URL fragments (`https://anything/#phantom:…`), so they can travel as QR codes
or web links without the fragment ever reaching a server.

### Live testnet proof (Esmeralda, 2026‑10‑12)

The full flow ran on the public Esmeralda testnet: L1 burn → Ootle claim → private funding → seal → zero‑balance open → double‑open rejected → expiry → reclaim through the hidden refund branch.
Full terminal capture: [`docs/live-run-output.txt`](docs/live-run-output.txt).

| step | tx | where / when | public link |
|---|---|---|---|
| L1 burn 200 tXTM to the walletd claim key | wallet tx `15835725866294681451`, kernel excess `a4d9fd31…9505` | L1 block **972736** | [textexplore kernel search](https://textexplore-esmeralda.tari.com/search_kernels?nonces=24892becc5184b1e806ece017c5462e34f71bd17214ff1e622415acc7beebd70&signatures=40a099d58b46e6495712006e75ea3dbba005aad210f6b7522383baa5aace3309) |
| Ootle `ClaimBurn` (auto, `tari_ootle_walletd`) | `b878a9103fc17ced2725038152eedae1a78c8a2a9b86213cb9cdbfa116aba85b` | epoch 12160 · Commit | [Veil](https://explorer.tari.mw/tx/b878a9103fc17ced2725038152eedae1a78c8a2a9b86213cb9cdbfa116aba85b) · [indexer](https://ootle-indexer-a.tari.com/transactions/b878a9103fc17ced2725038152eedae1a78c8a2a9b86213cb9cdbfa116aba85b/result) |
| stealth transfer 60 tTARI → `phantom address` | `a160f3f7d7ad326d2f92467e3d6df74b02f3352e5d5c67c7efc924c5e37ff026` | epoch 12160 · Commit | [Veil](https://explorer.tari.mw/tx/a160f3f7d7ad326d2f92467e3d6df74b02f3352e5d5c67c7efc924c5e37ff026) · [indexer](https://ootle-indexer-a.tari.com/transactions/a160f3f7d7ad326d2f92467e3d6df74b02f3352e5d5c67c7efc924c5e37ff026/result) |
| `phantom seal --amount 5 --count 2 --expires-in 1` (funded from the stealth UTXO) | `433611281465314a3c96a0b93566d6f0dd9e3713fde64bbb984b067310224805` | epoch 12160 · Commit · expiry 12161 | [Veil](https://explorer.tari.mw/tx/433611281465314a3c96a0b93566d6f0dd9e3713fde64bbb984b067310224805) · [indexer](https://ootle-indexer-a.tari.com/transactions/433611281465314a3c96a0b93566d6f0dd9e3713fde64bbb984b067310224805/result) |
| `phantom open` #0 into a brand‑new, zero‑balance key → 4.95 TARI | `c9855b67b8d72dc7b0bdd1600fe7665607247a2f7bd51218239f3a99d7206fd0` | epoch 12160 · Commit | [Veil](https://explorer.tari.mw/tx/c9855b67b8d72dc7b0bdd1600fe7665607247a2f7bd51218239f3a99d7206fd0) · [indexer](https://ootle-indexer-a.tari.com/transactions/c9855b67b8d72dc7b0bdd1600fe7665607247a2f7bd51218239f3a99d7206fd0/result) |
| second `open` of #0 | — | rejected: “already opened or reclaimed” | — |
| `phantom reclaim` #1 after expiry (hidden refund leaf) → 4.95 TARI back | `06d79409d49b842169c5eaa78dfad1dcef210f20ea1a3bf09867ffc46d23f8fb` | epoch 12161 · Commit | [Veil](https://explorer.tari.mw/tx/06d79409d49b842169c5eaa78dfad1dcef210f20ea1a3bf09867ffc46d23f8fb) · [indexer](https://ootle-indexer-a.tari.com/transactions/06d79409d49b842169c5eaa78dfad1dcef210f20ea1a3bf09867ffc46d23f8fb/result) |

Notes: the L1 wallet ran against the public base‑node RPC (`-p wallet.http_server_url=https://rpc.esmeralda.tari.com`), so no local node or mining was needed (the L1 tXTM came from the community faucet). An earlier 200 tXTM burn (wallet tx `3077477363113607069`, block 972709) was mis‑sent without a claim key and is unclaimable. The public indexer now rejects `per_shard_limit` 2000 on `/utxos/stream` and sometimes cuts streams short, so `scan.rs` uses 500, resumes until every shard has reported, and accepts `PHANTOM_UTXOS=<utxo ids>` as an escape hatch (used for the seal above).

The documented way to get testnet TARI without the faucet is to **burn Esmeralda Minotari (L1) and claim it
on Ootle** ([official guide](https://ootle.tari.com/guides/burn-minotari/)). See
[`docs/ESMERALDA.md`](docs/ESMERALDA.md) for the full, unattended route this project uses: sync an Esmeralda
L1 node, SHA3x-mine a block, burn, auto-claim with `tari_ootle_walletd`, stealth-transfer to the Phantom
sender, then `seal → open → reclaim`.

## Design notes

* **Link format:** `phantom:` + base58(`version ‖ network ‖ flags ‖ envelope seed ‖ commitment ‖
  refund pubkey ‖ expiry ‖ checksum`). The seed derives the *ghost* stealth wallet (view + account key)
  the envelope is encrypted to, and the claim key. The refund **public** key and expiry are included so
  the holder can recompute the condition root and verify the envelope before trusting it.
* **Funding:** `utxos` spends the sender's stealth UTXOs (found by scanning with the view key) → private;
  `account`/`faucet` are revealed inputs (handy for testing, but they link the seal to an account).
* **Fees:** the claim/reclaim spends a stealth input and creates one stealth output + a revealed fee
  bucket; the 0.05 TARI default fee is generous for Esmeralda.
* **Crash safety:** the CLI journals envelopes *before* submitting and rolls the journal back only when the
  network definitively rejected the transaction.

### What still leaks (honest threat model)

* The transaction graph: a seal and its later claim are linked (the claim spends that UTXO). The *claim
  key* that signs is a one‑time key, so it says nothing about the recipient.
* Timing and fees are public. Revealed funding (`account`/`faucet`) links the sender.
* After an envelope is used, the branch that was used (one leaf) becomes public; the other stays hidden.
* Riddle links are only as strong as the answer: Argon2id makes guessing slow, not impossible.
  Use riddles whose answers aren't guessable from a dictionary if real value is at stake.
* Anyone holding a plain link can claim it — treat links like cash.

## Layout

```
src/keys.rs      domain-separated key derivation, Argon2id riddle keys
src/link.rs      phantom: link encoding with checksum
src/envelope.rs  condition tree (claim / hidden refund leaf) and the stealth output
src/ops.rs       seal / inspect / open / reclaim transaction builders (ootle-rs)
src/scan.rs      stealth UTXO scanning with the view key
src/sim.rs       ootle-rs provider backed by the in-process Ootle engine (feature `sim`)
src/main.rs      `phantom` CLI
tests/           end-to-end engine tests
examples/        the haunted demo
```

## License

MIT. Written for the Tari October 2026 build contest, with help from an AI coding agent.
