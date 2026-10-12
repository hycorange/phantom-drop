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
    // Escape hatch when the indexer's UTXO stream is unreliable: test known UTXO substate ids
    // (e.g. the outputs of a funding transaction) directly, comma-separated in PHANTOM_UTXOS.
    if let Ok(ids) = std::env::var("PHANTOM_UTXOS") {
        let mut owned = Vec::new();
        for id in ids.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            let substate_id: SubstateId = id
                .parse()
                .map_err(|e| anyhow::anyhow!("bad substate id {id}: {e:?}"))?;
            let substate = provider.fetch_substate(substate_id.clone()).await?;
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
        return Ok(owned);
    }
    let network = key.network();
    let num_preshards = provider.get_num_preshards().await?;
    let mut cursor = ShardCursor::genesis(num_preshards);
    let mut candidates = Vec::new();
    // The public indexer may cut a stream short; keep resuming from the cursor until every shard
    // has sent its EndOfShard watermark.
    let mut done_shards = std::collections::HashSet::new();
    let mut passes = 0u32;

    loop {
        let request = StealthUtxoWatchRequest {
            resource_address: TARI_TOKEN,
            from_epoch,
            shard_state_versions: cursor.to_pairs(),
            unspent_only: true,
            per_shard_limit: 500,
        };
        let mut stream = Box::pin(provider.watch_stealth_utxos(request).into_stream());
        let mut more = false;
        while let Some(frame) = stream.next().await {
            let frame = frame?;
            match frame {
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
                } => {
                    done_shards.insert(format!("{shard:?}"));
                    cursor.observe(shard, max_state_version)
                }
                _ => {}
            }
        }
        passes += 1;
        let complete = done_shards.len() as u64 >= u64::from(u32::from(num_preshards));
        if (!more && complete) || passes >= 200 {
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
