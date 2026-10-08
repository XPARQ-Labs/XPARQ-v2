//! Canonical ledger state and rollback journal types.

use super::utxo::{self, CoinUtxo};

use crate::{
    monetary::coin::{CoinShare, Zeno},
    program::{ProgramJournal, ProgramRegistry},
};

use borsh::{BorshDeserialize, BorshSerialize};

#[derive(BorshSerialize, BorshDeserialize, Debug, Clone, Default, PartialEq, Eq)]
pub struct LedgerState {
    pub utxos: utxo::UtxoSet,
    pub coin: CoinRecord,
    pub programs: ProgramRegistry,
    pub extensions: crate::program::system::script::state::ExtensionState,
}

impl LedgerState {
    pub const fn utxos(&self) -> &utxo::UtxoSet {
        &self.utxos
    }
}

#[derive(BorshSerialize, BorshDeserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CoinRecord {
    pub total_mined: Zeno,
    pub total_burned: Zeno,
}

impl CoinRecord {
    pub fn supply(&self) -> Option<Zeno> {
        self.total_mined.checked_sub(self.total_burned)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct CoinRollbackJournal {
    pub(crate) consumed_coins: Vec<(CoinShare, CoinUtxo)>,
    pub(crate) created_coin_ids: Vec<CoinShare>,
    pub(crate) mined: Zeno,
    pub(crate) burned: Zeno,
}

#[derive(BorshSerialize, BorshDeserialize, Debug, Clone, Default, PartialEq, Eq)]
pub struct StateRollbackJournal {
    pub coin: Option<CoinRollbackJournal>,
    pub program: Option<ProgramJournal>,
    pub extension: Option<crate::program::system::asset_program::state::AssetJournal>,
}

impl StateRollbackJournal {
    pub const fn protocol_burn(&self) -> Zeno {
        match &self.coin {
            Some(journal) => journal.burned,
            None => Zeno::ZERO,
        }
    }
}

#[cfg(test)]
mod extension_commitment_tests {
    use super::*;
    use crate::program::system::asset_program::{
        asset::Unit,
        state::ExecutionContext,
        type_::{AssetCall, Register},
    };
    use crypto::{ProgramId, StateRoot};

    #[test]
    fn extension_state_is_committed_serialized_and_supply_checked() {
        let mut state = LedgerState::default();
        assert_eq!(state.application_state_root().unwrap(), StateRoot::ZERO);
        let call = AssetCall::Register(Register {
            name: "ROOT".into(),
            max_supply: Unit::from_units(100),
            initial_mint: Unit::from_units(10),
            mint_authority: crate::common::Owner::Program(ProgramId::ZERO),
            nonce: 1,
        });
        let journal = state
            .extensions
            .assets
            .apply(
                &call,
                ExecutionContext {
 actor: crate::common::Owner::Program(ProgramId::ZERO),
                    commitment: [3; 32],
                },
            )
            .unwrap();
        let root = state.application_state_root().unwrap();
        assert_ne!(root, StateRoot::ZERO);
        state.validate_supply_invariants().unwrap();
        let restored = LedgerState::try_from_slice(&borsh::to_vec(&state).unwrap()).unwrap();
        assert_eq!(restored, state);
        assert_eq!(restored.application_state_root().unwrap(), root);
        let share = state.extensions.assets.shares.values_mut().next().unwrap();
        share.amount = Unit::from_units(9);
        assert_ne!(state.application_state_root().unwrap(), root);
        assert!(state.validate_supply_invariants().is_err());
        state.extensions.assets.rollback(journal);
        assert_eq!(state, LedgerState::default());
        assert_eq!(state.application_state_root().unwrap(), StateRoot::ZERO);
    }
}
