//! Protocol application set installed by the runtime.

use kernel::{
    crypto::Address,
    ledger::StateError,
    monetary::coin::CoinShare,
    program::{
        application::{ApplicationExecutor, AssetHost},
        system::{
            asset_program::{asset::AssetError, type_::AssetCall},
            coin_program::{CoinHost, TransferError},
        },
    },
};

#[derive(Debug, Clone, Copy, Default)]
pub struct SystemApplications;

impl ApplicationExecutor for SystemApplications {
    fn execute_coin(
        &self,
        host: &mut dyn CoinHost<CoinId = CoinShare, Error = StateError>,
        inputs: &[CoinShare],
        outputs: &[(Address, u64)],
        miner: Address,
        miner_fee: u64,
    ) -> Result<(), TransferError<StateError>> {
        crate::coin_program::execute_transfer(host, inputs, outputs, miner, miner_fee)
    }

    fn execute_asset(&self, call: &AssetCall, host: &mut dyn AssetHost) -> Result<(), AssetError> {
        crate::asset_program::execute(call, host)
    }
}
