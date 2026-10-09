# Running Phantom Drop live on Esmeralda

## 1. Get testnet TARI

| route | status (2026‑10‑09) |
|---|---|
| Public faucet (`phantom seal --fund faucet`, wallet "Claim Testnet Funds") | ❌ stuck at 999.65 TARI, below its fixed 1000 TARI take; check with `phantom status` |
| Burn Esmeralda Minotari (L1) → claim on Ootle ([guide](https://ootle.tari.com/guides/burn-minotari/)) | ✅ official route; needs L1 tXTM |
| Mine L1 tXTM yourself | ✅ Esmeralda L1 SHA3x hash rate is about 1 MH/s, so a CPU miner finds blocks |

Tools: Tari suite v6.1.0 for esmeralda (`minotari_node`, `minotari_console_wallet`, `minotari_miner`),
`tari_ootle_walletd` v0.45.0, `grpcurl` and the protos from `applications/minotari_app_grpc/proto`.

```bash
export TARI_BASE_DIR=$PWD/data
# L1 node (pruned is enough). Mining needs these gRPC methods enabled:
minotari_node --network esmeralda -n --grpc-enabled --mining-enabled \
  -p base_node.storage.pruning_horizon=2000 \
  -p 'base_node.grpc_server_allow_methods="get_version","identify","get_tip_info","get_sync_progress","get_new_block_template","get_new_block","submit_block","get_network_difficulty","get_constants"'
# L1 wallet with gRPC (burns go through gRPC)
MINOTARI_WALLET_PASSWORD=… minotari_console_wallet --network esmeralda -n --grpc-enabled --burn-proof-out $PWD/burn_proofs
# once the node reports DONE (header sync takes several hours on one core):
minotari_miner --network esmeralda -n -p miner.wallet_payment_address=<one-sided address>
# Ootle wallet daemon watching the same proof directory (auto-claim is on by default)
tari_ootle_walletd --network esmeralda -p ootle_wallet_daemon.burn_proof_dir=$PWD/burn_proofs run
# burn to the walletd account public key (hex → base64)
grpcurl -plaintext -import-path proto -proto wallet.proto \
  -d '{"amount":200000000,"fee_per_gram":5,"claim_public_key":"<b64>"}' 127.0.0.1:18143 tari.rpc.Wallet/CreateBurnTransaction
```

After the walletd claims the burn, send TARI to `phantom address` with `accounts.stealth_transfer`. The
sender then holds stealth UTXOs, so the default `--fund utxos` keeps the seal unlinkable to any account.

## 2. The run

```bash
phantom status
phantom balance --from-epoch <epoch before funding>
phantom seal --amount 5 --count 2 --expires-in 2 --from-epoch <same>   # two links
phantom peek  <link #0>
phantom open  <link #0> --new-wallet recipient.key   # recipient has zero balance
phantom open  <link #0> --new-wallet other.key       # fails: already opened
# …wait two epochs…
phantom reclaim                                      # #1 comes back through the hidden refund leaf
phantom list
```
