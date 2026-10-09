<!-- Draft reply for https://community.tari.com/t/october-build-contest-thread-spooky-secrets/396
     Fill in the three <PLACEHOLDERS> before posting. Do not post until the repo is public and the promo post exists. -->

## 👻 Phantom Drop — haunted claim links for stealth TARI

**Code (MIT):** https://gitlab.com/hycorange/phantom-drop
**Tari payment address:** 12AkA1PP1qJUYK48GnfnmjsRcMqkou8XTVjSxF3vjBtf8BbLmLQsAkKzYyCMMPEq4Hp4dQuEMHDTGGvwHMLE2L6QeWB
**Announcement:** <BLUESKY_OR_REDDIT_POST_URL>

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
- The engine tests and demo pass. The CLI connects to the public Esmeralda indexer and builds and signs seal transactions; <UPDATE: live testnet tx ids here if you run one>.
- Built with an AI coding agent, then reviewed by me. All code is new for this contest.

Feedback welcome. Happy Halloween 🎃
