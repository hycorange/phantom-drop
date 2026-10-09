# Promo post drafts (post ONE of these publicly, then link it in the forum reply)

## Bluesky (≤300 chars)

👻 Phantom Drop: turn TARI into a secret link. Anyone with the link can claim it privately, even with an empty wallet. Unopened links die at "midnight" and quietly return to the sender. Riddle mode: first to solve it wins.
Built on Tari Ootle stealth UTXOs for the October build contest 🎃
https://gitlab.com/hycorange/phantom-drop

## Reddit — r/tari

**Title:** Phantom Drop: private claim links for TARI on Ootle (October build contest entry)

**Body:**
For the Spooky Secrets contest I built Phantom Drop, a CLI and Rust library that turns TARI into a secret link.

- Whoever holds the link can claim the funds as a stealth UTXO. They don't need a wallet or any balance first, because the claim pays its fee out of the envelope.
- Links expire at a chosen epoch. After that only the sender can reclaim, through a refund branch that stays hidden on chain unless it's used.
- Riddle mode: the claim key comes from the answer, so you can post a link publicly and the first person to solve it gets the funds.
- On chain an envelope is only a commitment, ciphertext and a Merkle root. No amount, owner, keys or expiry.

It only uses Ootle's built-in primitives (stealth outputs, spend-condition trees, epoch timelocks), with no custom template. The tests run against the real Ootle engine, and the README explains what still leaks.

Code (MIT): https://gitlab.com/hycorange/phantom-drop
Forum entry: <FORUM_POST_URL_OR_CONTEST_THREAD>

Feedback is welcome, especially on the threat model.
