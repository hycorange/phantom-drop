# 👻 Phantom Drop — haunted claim links for stealth TARI on Tari Ootle

[![pipeline](https://gitlab.com/hycorange/phantom-drop/badges/main/pipeline.svg)](https://gitlab.com/hycorange/phantom-drop/-/pipelines)

Repository: <https://gitlab.com/hycorange/phantom-drop>

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

CI: [`.gitlab-ci.yml`](.gitlab-ci.yml) runs `cargo fmt --check`, `cargo clippy -D warnings`, `cargo test`
and the haunted demo (its output is saved as a job artifact) on GitLab.

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

### Live Esmeralda run: pending (faucet depleted)

> **Status 2026‑10‑09:** the live testnet run has **not happened yet**. `phantom status` shows the public
> Esmeralda faucet holds **999.65 TARI**, just under the fixed 1000 TARI per take, so `IFaucet::take` fails
> and the faucet can't fund anyone. `status` and `seal --dry-run` (build + sign, no submit) work against
> the public indexer. Everything the live run would exercise is covered by the engine tests above.
> Transaction IDs will be added here once the run completes.

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
