//! XPQ application execution using the restricted kernel coin host.
pub use kernel::program::system::coin_program::*;
use kernel::{common::Owner, monetary::coin::CoinShare};
/// Execute a validated transfer, including funding for other Program calls.
/// The host must validate authorization and provide atomic commit/rollback.
pub fn execute_transfer<H: CoinHost + ?Sized>(
    host: &mut H,
    inputs: &[CoinShare],
    outputs: &[(Owner, u64)],
    miner: kernel::crypto::Address,
    miner_fee: u64,
) -> Result<(), TransferError<H::Error>> {
    let mut input_total = 0u64;
    for id in inputs {
        input_total = input_total
            .checked_add(host.input_amount(id).map_err(TransferError::Host)?)
            .ok_or(TransferError::AmountOverflow)?;
    }
    let output_total = outputs.iter().try_fold(0u64, |sum, (_, amount)| {
        sum.checked_add(*amount)
            .ok_or(TransferError::AmountOverflow)
    })?;
    let burn = input_total
        .checked_sub(output_total)
        .and_then(|remaining| remaining.checked_sub(miner_fee))
        .ok_or(TransferError::InvalidBalance)?;
    for id in inputs {
        host.consume(*id).map_err(TransferError::Host)?;
    }
    for (index, (owner, amount)) in outputs.iter().enumerate() {
        host.create(
            u32::try_from(index).map_err(|_| TransferError::OutputIndexOverflow)?,
            *owner,
            *amount,
        )
        .map_err(TransferError::Host)?;
    }
    if miner_fee != 0 {
        host.create(
            u32::try_from(outputs.len()).map_err(|_| TransferError::OutputIndexOverflow)?,
            Owner::Address(miner),
            miner_fee,
        )
        .map_err(TransferError::Host)?;
    }
    host.burn(burn).map_err(TransferError::Host)
}
