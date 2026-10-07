//! Native XPQ transfer execution through a kernel-owned coin host.
use crate::common::Owner;
pub const TRANSFER: u8 = 1;
use crate::monetary::coin::CoinShare;

/// Restricted capability implemented by the kernel. The program never receives
/// the ledger, coin types, or mutable access to monetary counters.
pub trait CoinHost {
    type Error;

    fn input_amount(&self, id: &CoinShare) -> Result<u64, Self::Error>;
    fn consume(&mut self, id: CoinShare) -> Result<(), Self::Error>;
    fn create(&mut self, index: u32, owner: Owner, amount: u64) -> Result<(), Self::Error>;
    fn burn(&mut self, amount: u64) -> Result<(), Self::Error>;
}

#[derive(Debug, PartialEq, Eq)]
pub enum TransferError<E> {
    Host(E),
    InvalidBalance,
    AmountOverflow,
    OutputIndexOverflow,
}

/// XPQ.Transfer uses the envelope funding field as its transfer data.
/// An empty payload prevents an ambiguous second set of inputs or outputs.
pub fn decode(
    opcode: u8,
    payload: &[u8],
) -> Result<(), crate::program::system::script::opcode::ProgramError> {
    use crate::program::system::script::opcode::ProgramError;
    if opcode != TRANSFER {
        return Err(ProgramError::UnknownOpcode);
    }
    if !payload.is_empty() {
        return Err(ProgramError::InvalidPayload);
    }
    Ok(())
}

pub fn transfer_call() -> crate::program::system::script::call::ProgramCall {
    crate::program::system::script::call::ProgramCall {
        program: crate::program::system::script::call::SystemProgramId::XPQ,
        opcode: TRANSFER,
        payload: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_transfer_rejects_unknown_methods_and_duplicate_payload_data() {
        assert!(decode(TRANSFER, &[]).is_ok());
        assert!(decode(TRANSFER, &[0]).is_err());
        assert!(decode(2, &[]).is_err());
    }
}
