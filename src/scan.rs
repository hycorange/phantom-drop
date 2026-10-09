//! Finding the stealth UTXOs a key owns, by streaming the indexer's UTXO updates and testing each
//! output's 32-bit tag before fetching and trial-decrypting the few candidates.

use futures::StreamExt;
use ootle_byte_type::FromByteType;
use ootle_rs::{
    Epoch,
    crypto::kdfs::utxo_tag_stealth_dh,
    keys::OotleSecretKey,
    provider::{IndexerProvider, ShardCursor, StealthUtxoFrame, StealthUtxoWatchRequest},
    template_types::{UtxoAddress, constants::TARI_TOKEN, crypto::PedersenCommitmentBytes},
    wallet::NetworkWallet,
};
use tari_crypto::ristretto::RistrettoPublicKey;
use tari_ootle_common_types::engine_types::substate::{Substate, SubstateId};

use crate::ops::try_decrypt_substate;

#[derive(Debug, Clone)]
pub struct OwnedUtxo {
    pub commitment: PedersenCommitmentBytes,
    pub value: u64,
    pub memo: Option<String>,
}

pub async fn scan_owned<W: NetworkWallet + Send + Sync>(
    provider: &IndexerProvider<W>,
    key: &OotleSecretKey,
    from_epoch: Epoch,
) -> anyhow::Result<Vec<OwnedUtxo>> {
    let network = key.network();
    let num_preshards = provider.get_num_preshards().await?;
    let mut cursor = ShardCursor::genesis(num_preshards);
    let mut candidates = Vec::new();

    loop {
        let request = StealthUtxoWatchRequest {
            resource_address: TARI_TOKEN,
            from_epoch,
            shard_state_versions: cursor.to_pairs(),
            unspent_only: true,
            per_shard_limit: 2000,
        };
        let mut stream = Box::pin(provider.watch_stealth_utxos(request).into_stream());
        let mut more = false;
        while let Some(frame) = stream.next().await {
            match frame? {
                StealthUtxoFrame::StartOfShard { has_more, .. } => more |= has_more,
                StealthUtxoFrame::Unspent { tag, public_nonce } => {
                    let Ok(nonce) = FromByteType::try_from_byte_type(&public_nonce) else {
                        continue;
                    };
                    let nonce: RistrettoPublicKey = nonce;
                    if utxo_tag_stealth_dh(network, &nonce, key.view_only_secret(), &TARI_TOKEN)
                        == tag
                    {
                        candidates.push((tag, public_nonce));
                    }
                }
                StealthUtxoFrame::EndOfShard {
                    shard,
                    max_state_version,
                } => cursor.observe(shard, max_state_version),
                _ => {}
            }
        }
        if !more {
            break;
        }
    }

    let mut owned = Vec::new();
    for (id, utxo) in provider
        .fetch_unspent_utxos(TARI_TOKEN, &candidates)
        .await?
    {
        let substate_id = SubstateId::from(UtxoAddress::new(TARI_TOKEN, id));
        let substate = Substate::new(tari_ootle_common_types::SubstateVersion::ZERO, utxo);
        if let Some((commitment, value, memo)) =
            try_decrypt_substate(&substate_id, &substate, key.view_only_secret())
        {
            owned.push(OwnedUtxo {
                commitment,
                value,
                memo,
            });
        }
    }
    Ok(owned)
}
