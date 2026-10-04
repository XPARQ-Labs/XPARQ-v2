//! Asset application dispatch onto checked kernel monetary operations.

pub use kernel::program::system::asset_program::*;

use kernel::program::application::AssetHost;

pub fn execute(call: &type_::AssetCall, host: &mut dyn AssetHost) -> Result<(), asset::AssetError> {
    use type_::AssetCall;
    match call {
        AssetCall::Register(call) => host.register(call),
        AssetCall::Mint(call) => host.mint(call),
        AssetCall::Transfer(call) => host.transfer(call),
        AssetCall::Burn(call) => host.burn(call),
    }
}
