use crate::monetary::coin::Zeno;
use crypto::canonical_bytes;

use crate::{
    common::{ChainContext, Height},
    consensus::{
        ProgramConsensusError, ProtocolBurn, StateTransitionWeight, created_coin_output_count,
        validate_exact_burn,
    },
    ledger::LedgerState,
    operation::{AuthorizedDeployProgram, BlockOperation},
    program::{AuthorizationCommitment, DeployError, ProgramId, deploy_program},
};

#[derive(Debug, Clone)]
pub struct PreparedDeploy {
    pub(crate) signed: AuthorizedDeployProgram,
    pub(crate) commitment: AuthorizationCommitment,
    pub(crate) program_id: ProgramId,
    pub(crate) height: Height,
    pub required_burn: crate::monetary::coin::Zeno,
}

#[derive(Debug)]
pub enum DeployConsensusError {
    Deploy(DeployError),
    Program(ProgramConsensusError),
}

pub fn validate_deploy(
    signed: AuthorizedDeployProgram,
    chain: ChainContext,
    height: Height,
    state: &LedgerState,
) -> Result<PreparedDeploy, DeployConsensusError> {
    signed
        .deploy
        .validate_structure()
        .map_err(DeployConsensusError::Deploy)?;
    signed
        .payment
        .validate()
        .map_err(|_| DeployConsensusError::Deploy(DeployError::InvalidPayment))?;
    if signed.deploy.owner != signed.payment.signer {
        return Err(DeployConsensusError::Deploy(DeployError::InvalidPayment));
    }
    let commitment = signed
        .commitment(chain)
        .map_err(|_| DeployConsensusError::Deploy(DeployError::Encoding))?;
    if !signed
        .authorization
        .verify_commitment(signed.deploy.owner, &commitment, height.0)
    {
        return Err(DeployConsensusError::Deploy(
            DeployError::InvalidAuthorization,
        ));
    }
    let (program_id, required_burn) = quote_deploy_burn(&signed, height, state)?;
    let (inputs, outputs) = signed
        .payment
        .coin_parts()
        .ok_or(DeployConsensusError::Deploy(DeployError::InvalidPayment))?;
    let fee = signed.payment.charges.miner_fee;
    let actual =
        super::program_call::validate_coin_inputs(inputs, outputs, fee, signed.deploy.owner, state)
            .map_err(DeployConsensusError::Program)?;
    validate_exact_burn(actual, required_burn)
        .map_err(ProgramConsensusError::Burn)
        .map_err(DeployConsensusError::Program)?;
    Ok(PreparedDeploy {
        signed,
        commitment,
        program_id,
        height,
        required_burn,
    })
}

/// Quote archival and registry growth burn before the owner signs the payment.
pub fn quote_deploy_burn(
    signed: &AuthorizedDeployProgram,
    height: Height,
    state: &LedgerState,
) -> Result<(ProgramId, Zeno), DeployConsensusError> {
    signed
        .deploy
        .validate_structure()
        .map_err(DeployConsensusError::Deploy)?;
    signed
        .payment
        .validate()
        .map_err(|_| DeployConsensusError::Deploy(DeployError::InvalidPayment))?;
    if signed.deploy.owner != signed.payment.signer {
        return Err(DeployConsensusError::Deploy(DeployError::InvalidPayment));
    }
    let size = canonical_bytes(&BlockOperation::DeployProgram(Box::new(signed.clone())))
        .map_err(|_| DeployConsensusError::Deploy(DeployError::Encoding))?
        .len();
    if size > crate::blockchain::MAX_OPERATION_SIZE {
        return Err(DeployConsensusError::Deploy(DeployError::ProgramTooLarge));
    }
    let mut registry = state.programs.clone();
    let before = canonical_bytes(&registry)
        .map_err(|_| DeployConsensusError::Deploy(DeployError::Encoding))?
        .len();
    let (program_id, _) = deploy_program(&mut registry, signed.deploy.clone(), height)
        .map_err(DeployConsensusError::Deploy)?;
    let after = canonical_bytes(&registry)
        .map_err(|_| DeployConsensusError::Deploy(DeployError::Encoding))?
        .len();
    let growth = u64::try_from(
        after
            .checked_sub(before)
            .ok_or(DeployConsensusError::Deploy(DeployError::Encoding))?,
    )
    .map_err(|_| DeployConsensusError::Deploy(DeployError::Encoding))?;
    let (inputs, outputs) = signed
        .payment
        .coin_parts()
        .ok_or(DeployConsensusError::Deploy(DeployError::InvalidPayment))?;
    let fee = signed.payment.charges.miner_fee;
    let transition = StateTransitionWeight {
        created_coin_utxos: created_coin_output_count(outputs)
            .map_err(ProgramConsensusError::Burn)
            .map_err(DeployConsensusError::Program)?
            + u64::from(!fee.is_zero()),
        consumed_coin_utxos: inputs.len() as u64,
        created_state_weight: growth,
    };
    let required_burn = ProtocolBurn::for_program_call(transition, size as u64)
        .and_then(ProtocolBurn::total)
        .map_err(ProgramConsensusError::Burn)
        .map_err(DeployConsensusError::Program)?;
    Ok((program_id, required_burn))
}
