use crate::program::system::asset_program::{self, type_::AssetCall};

use super::{
    call::{ProgramCall, SystemProgramId},
    opcode::ProgramError,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodedProgramCall {
    XpqTransfer,
    Asset(AssetCall),
    Vm([u8; 32]),
}

/// Decode a call before any state transition. This does not execute it.
pub fn decode_program(call: &ProgramCall) -> Result<DecodedProgramCall, ProgramError> {
    match call.program {
        SystemProgramId::XPQ => {
            crate::program::system::coin_program::decode(call.opcode, &call.payload)?;
            Ok(DecodedProgramCall::XpqTransfer)
        }
        SystemProgramId::ASSET => {
            asset_program::decode(call.opcode, &call.payload).map(DecodedProgramCall::Asset)
        }
        SystemProgramId::VM if call.opcode == 0 => {
            let id = call
                .payload
                .as_slice()
                .try_into()
                .map_err(|_| ProgramError::InvalidPayload)?;
            Ok(DecodedProgramCall::Vm(id))
        }
        _ => Err(ProgramError::UnknownProgram),
    }
}
