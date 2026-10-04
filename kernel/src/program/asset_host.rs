//! Bound asset capability: the kernel checks ownership and supply and records
//! rollback, while the application selects the requested monetary operation.

use super::{
    application::{ApplicationExecutor, AssetHost},
    system::asset_program::{
        asset::AssetError,
        state::{AssetJournal, AssetState, ExecutionContext},
        type_::{AssetCall, Burn, Mint, Register, Transfer},
    },
};

struct BoundAssetHost<'a> {
    state: &'a mut AssetState,
    expected: &'a AssetCall,
    context: ExecutionContext,
    journal: Option<AssetJournal>,
}

impl BoundAssetHost<'_> {
    fn apply(&mut self, operation: AssetCall) -> Result<(), AssetError> {
        if &operation != self.expected || self.journal.is_some() {
            return Err(AssetError::InvalidProgram);
        }
        self.journal = Some(self.state.apply(&operation, self.context)?);
        Ok(())
    }
}

impl AssetHost for BoundAssetHost<'_> {
    fn register(&mut self, call: &Register) -> Result<(), AssetError> {
        self.apply(AssetCall::Register(call.clone()))
    }
    fn mint(&mut self, call: &Mint) -> Result<(), AssetError> {
        self.apply(AssetCall::Mint(call.clone()))
    }
    fn transfer(&mut self, call: &Transfer) -> Result<(), AssetError> {
        self.apply(AssetCall::Transfer(call.clone()))
    }
    fn burn(&mut self, call: &Burn) -> Result<(), AssetError> {
        self.apply(AssetCall::Burn(call.clone()))
    }
}

pub(crate) fn execute_asset(
    applications: &dyn ApplicationExecutor,
    state: &mut AssetState,
    call: &AssetCall,
    context: ExecutionContext,
) -> Result<AssetJournal, AssetError> {
    let mut staged = state.clone();
    let mut host = BoundAssetHost {
        state: &mut staged,
        expected: call,
        context,
        journal: None,
    };
    applications.execute_asset(call, &mut host)?;
    let journal = host.journal.take().ok_or(AssetError::InvalidProgram)?;
    *state = staged;
    Ok(journal)
}
