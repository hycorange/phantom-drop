//! An in-process Ootle "network" for tests and demos: the real Ootle engine
//! (`tari_template_test_tooling`) behind an `ootle-rs` [`Provider`], so the exact transaction
//! builders the CLI uses against a live indexer are executed — Bulletproofs, balance proofs,
//! condition trees, timelocks, access rules and fees included — without any network.

use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Weak},
};

use ootle_rs::{
    Address, Epoch, Network,
    keys::OotleSecretKey,
    provider::{Provider, ProviderResult, WalletProvider, WantInput},
    wallet::{NetworkWallet, OotleWallet},
};
use tari_ootle_common_types::engine_types::substate::{Substate, SubstateId};
use tari_ootle_transaction::{Transaction, UnsignedTransaction};
use tari_template_test_tooling::{
    TemplateTest,
    engine_types::virtual_substate::{VirtualSubstate, VirtualSubstateId},
};

use crate::ops::{try_decrypt_substate, wallet_for};

pub const SIM_NETWORK: Network = Network::LocalNet;

/// A provider whose view of the chain is a snapshot of the simulator's state.
pub struct EngineProvider {
    wallet: OotleWallet,
    substates: HashMap<SubstateId, Substate>,
}

impl Provider for EngineProvider {
    type Client = ();

    fn network(&self) -> Network {
        SIM_NETWORK
    }

    fn weak_client(&self) -> Weak<Self::Client> {
        Weak::new()
    }

    fn client_upgrade(&mut self) -> Option<Arc<Self::Client>> {
        None
    }

    fn default_signer_address(&self) -> &Address {
        self.wallet.default_address()
    }

    async fn resolve_input_want_list(
        &self,
        transaction: UnsignedTransaction,
        _want_list: &HashSet<WantInput>,
    ) -> ProviderResult<UnsignedTransaction> {
        // The simulator declares every substate as an input itself.
        Ok(transaction)
    }

    async fn fetch_substates<I: IntoIterator<Item = SubstateId> + Send>(
        &self,
        ids: I,
    ) -> ProviderResult<HashMap<SubstateId, Substate>> {
        Ok(ids
            .into_iter()
            .filter_map(|id| self.substates.get(&id).map(|s| (id, s.clone())))
            .collect())
    }
}

impl WalletProvider for EngineProvider {
    type Wallet = OotleWallet;

    fn wallet(&self) -> &OotleWallet {
        &self.wallet
    }

    fn wallet_mut(&mut self) -> &mut OotleWallet {
        &mut self.wallet
    }
}

/// The simulated chain.
pub struct Sim {
    pub engine: TemplateTest,
}

impl Default for Sim {
    fn default() -> Self {
        Self::new()
    }
}

impl Sim {
    /// Built-in templates (account, faucet) bootstrapped, fees enabled, epoch 10.
    pub fn new() -> Self {
        let mut engine = TemplateTest::new_builtin_only();
        engine.enable_fees();
        let mut sim = Self { engine };
        sim.set_epoch(10);
        sim
    }

    pub fn set_epoch(&mut self, epoch: u64) {
        self.engine.set_virtual_substate(
            VirtualSubstateId::CurrentEpoch,
            VirtualSubstate::CurrentEpoch(epoch),
        );
    }

    pub fn epoch(&self) -> u64 {
        self.engine.current_epoch().as_u64()
    }

    pub fn max_epoch(&self) -> Epoch {
        Epoch(self.epoch() + 10)
    }

    /// A provider over the current state, with `key` as its wallet.
    pub fn provider(&mut self, key: OotleSecretKey) -> EngineProvider {
        let substates = self
            .engine
            .get_state_store_mut()
            .iter()
            .map(|(id, s)| (id.clone(), s.clone()))
            .collect();
        EngineProvider {
            wallet: wallet_for(key),
            substates,
        }
    }

    /// Execute and commit. `Err` carries the engine's rejection reason.
    pub fn submit(&mut self, tx: Transaction) -> Result<(), String> {
        if !tx.verify_all_signatures() {
            return Err("invalid signature".into());
        }
        // Commits whatever the engine accepts (a fee-only accept is committed too, as on a real network).
        let result = self.engine.execute_and_commit_on_success(tx, vec![]);
        if let Some(reason) = result.finalize.result.any_reject() {
            return Err(format!("{reason:?}"));
        }
        Ok(())
    }

    /// Every unspent TARI UTXO addressed to `key`: `(value, memo)`.
    pub fn received(&mut self, key: &OotleSecretKey) -> Vec<(u64, Option<String>)> {
        self.engine
            .get_state_store_mut()
            .iter()
            .filter_map(|(id, s)| try_decrypt_substate(id, s, key.view_only_secret()))
            .map(|(_, v, m)| (v, m))
            .collect()
    }

    /// The JSON form of a substate as stored on-chain.
    pub fn substate_json(&mut self, id: &SubstateId) -> Option<String> {
        self.engine
            .get_state_store_mut()
            .iter()
            .find(|(i, _)| *i == id)
            .map(|(_, s)| serde_json::to_string(s).expect("substates serialize"))
    }
}
