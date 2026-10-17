<!-- Draft reply for https://community.tari.com/t/october-build-contest-thread-spooky-secrets/396
     Fill in the three <PLACEHOLDERS> before posting. Do not post until the repo is public and the promo post exists. -->
<!-- NOTE 2026-10-12: Discourse new-user limit = 5 links/post. Live submit uses forum-post-body.md (trimmed). Full link set remains below for later edit once trust level rises. -->


## 👻 Phantom Drop — haunted claim links for stealth TARI

**Code (MIT):** https://gitlab.com/hycorange/phantom-drop (GitHub mirror: https://github.com/hycorange/phantom-drop)
**CI:** [![CI](https://github.com/hycorange/phantom-drop/actions/workflows/ci.yml/badge.svg)](https://github.com/hycorange/phantom-drop/actions/workflows/ci.yml) (GitHub Actions: fmt + tests + haunted demo)
**Tari payment address:** 12AkA1PP1qJUYK48GnfnmjsRcMqkou8XTVjSxF3vjBtf8BbLmLQsAkKzYyCMMPEq4Hp4dQuEMHDTGGvwHMLE2L6QeWB
**Announcement:** https://bsky.app/profile/hycorange.bsky.social/post/3mxh4tpegms22

**What it is**
Phantom Drop turns TARI into a secret link. Whoever holds the link can open it and receive the funds as a stealth UTXO — even with no wallet and zero balance. If nobody opens it before an expiry epoch, the link dies and only the sender can take the money back. Optional riddle locks make the link useless without the answer, so you can post a link publicly: first to solve it wins.

**How it uses Ootle's privacy primitives (no custom template)**
- The envelope is a stealth output: Pedersen commitment + Bulletproof, encrypted value/memo, sent to a "ghost" stealth address derived from the link.
- It is locked by a 2‑leaf **MAST spend‑condition tree**, and only the 32‑byte root is on chain:
  - claim leaf: one‑time claim key signature ∧ `BeforeEpoch(E)`
  - refund leaf: sender's one‑time refund key ∧ `AfterEpoch(E)` — stays hidden unless the sender actually reclaims.
- Riddle mode derives the claim key from Argon2id(answer).
- The claim pays its fee out of the envelope itself, signed only by a one‑time key, so the recipient needs no funds and isn't linked to the claim.
- Up to 64 envelopes per seal transaction; funding from stealth UTXOs keeps the sender private.

On chain, a sealed envelope is just a commitment, a nonce, ciphertext and a Merkle root: no amount, no owner, no keys, no expiry.

**What's included**
- `phantom` CLI for Esmeralda: `init / address / status / balance / seal / peek / open / reclaim / list`
- Rust library on top of `ootle-rs`
- 5 end‑to‑end tests that run against the **real Ootle engine** in‑process (real proofs, timelocks, signatures, fees): zero‑balance claim, replay rejection, riddle `AccessDenied`, expiry/refund rules, plus a check that the on‑chain state contains no amount, keys or expiry
- `cargo run --example haunted_demo --features sim` — a narrated 5‑act story (Alice claims with an empty wallet, Mallory gets `AccessDenied` on the riddle, a leaked link dies at midnight, the sender reclaims through the hidden branch)
- README with the threat model and a list of what still leaks (tx graph, timing/fees, the leaf that gets used, riddle guessability)

**Status / honesty notes**
- The engine tests and demo pass. The CLI connects to the public Esmeralda indexer and builds and signs seal transactions.
- Live Esmeralda run: ✅ done 2026‑10‑12 (L1 burn → Ootle claim → seal → zero‑balance open → double‑open rejected → reclaim after expiry). Details below.

**Live testnet proof (Esmeralda)**
- L1 burn 200 tXTM: wallet tx 15835725866294681451, block 972736 — https://textexplore-esmeralda.tari.com/search_kernels?nonces=24892becc5184b1e806ece017c5462e34f71bd17214ff1e622415acc7beebd70&signatures=40a099d58b46e6495712006e75ea3dbba005aad210f6b7522383baa5aace3309
- Ootle ClaimBurn: https://explorer.tari.mw/tx/b878a9103fc17ced2725038152eedae1a78c8a2a9b86213cb9cdbfa116aba85b
- Stealth funding 60 tTARI → phantom sender: https://explorer.tari.mw/tx/a160f3f7d7ad326d2f92467e3d6df74b02f3352e5d5c67c7efc924c5e37ff026
- Seal 2 × 5 TARI (expiry epoch 12161): https://explorer.tari.mw/tx/433611281465314a3c96a0b93566d6f0dd9e3713fde64bbb984b067310224805
- Open #0 into a fresh zero‑balance key (4.95 TARI received): https://explorer.tari.mw/tx/c9855b67b8d72dc7b0bdd1600fe7665607247a2f7bd51218239f3a99d7206fd0
- Second open of #0 → rejected “already opened or reclaimed”
- Reclaim #1 after expiry via the hidden refund leaf: https://explorer.tari.mw/tx/06d79409d49b842169c5eaa78dfad1dcef210f20ea1a3bf09867ffc46d23f8fb
- Execution results (JSON) for each Ootle tx: https://ootle-indexer-a.tari.com/transactions/<tx id>/result · full terminal capture in docs/live-run-output.txt

Feedback welcome. Happy Halloween 🎃
