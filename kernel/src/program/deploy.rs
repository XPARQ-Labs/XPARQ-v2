use borsh::{BorshDeserialize, BorshSerialize};
use crypto::Address;

use crate::common::Height;

use super::registry::{ProgramHash, ProgramId, ProgramRecord, ProgramRegistry, RegistryError};

pub const MAX_PROGRAM_CODE_SIZE: usize = 1_048_576;

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct DeployProgram {
    pub owner: Address,
    pub nonce: u64,
    pub code: Vec<u8>,
}

impl DeployProgram {
    pub fn validate_structure(&self) -> Result<(), DeployError> {
        if self.code.is_empty() {
            return Err(DeployError::EmptyProgram);
        }

        if self.code.len() > MAX_PROGRAM_CODE_SIZE {
            return Err(DeployError::ProgramTooLarge);
        }

        super::vm::validate_code(&self.code).map_err(|_| DeployError::InvalidCode)?;

        Ok(())
    }
}

#[derive(BorshSerialize, BorshDeserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgramJournal {
    Deploy {
        program_id: ProgramId,
    },
    State {
        program_id: ProgramId,
        previous: i64,
    },
}

pub fn deploy_program(
    registry: &mut ProgramRegistry,
    deploy: DeployProgram,
    height: Height,
) -> Result<(ProgramId, ProgramJournal), DeployError> {
    deploy.validate_structure()?;

    let code_hash = ProgramHash::derive(&deploy.code).map_err(DeployError::Registry)?;

    let program_id =
        ProgramId::derive(deploy.owner, deploy.nonce, code_hash).map_err(DeployError::Registry)?;

    let record = ProgramRecord {
        code_hash,
        code: deploy.code,
        owner: deploy.owner,
        nonce: deploy.nonce,
        deployed_at: height,
        state_value: 0,
    };

    registry
        .insert(program_id, record)
        .map_err(DeployError::Registry)?;

    Ok((program_id, ProgramJournal::Deploy { program_id }))
}

pub fn rollback_program(
    registry: &mut ProgramRegistry,
    journal: ProgramJournal,
) -> Result<(), DeployError> {
    match journal {
        ProgramJournal::Deploy { program_id } => {
            registry
                .remove(&program_id)
                .ok_or(DeployError::MissingProgram)?;
        }
        ProgramJournal::State {
            program_id,
            previous,
        } => {
            registry
                .set_state(program_id, previous)
                .ok_or(DeployError::MissingProgram)?;
        }
    }

    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeployError {
    EmptyProgram,
    ProgramTooLarge,
    InvalidCode,
    InvalidPayment,
    InvalidAuthorization,
    Encoding,
    MissingProgram,
    Registry(RegistryError),
}

#[cfg(test)]
fn valid_code(value: u64) -> Vec<u8> {
    let mut code = b"XPVM".to_vec();
    code.extend_from_slice(&[1, 1, 0, 0, 0, 0, 0, 0, 0]);
    code.push(1);
    code.extend_from_slice(&value.to_le_bytes());
    code.push(3);
    code
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::{LedgerState, StateRollbackJournal};
    use crypto::Address;

    #[test]
    fn deploy_and_rollback_restore_registry() {
        let mut state = LedgerState::default();
        let before = state.clone();

        let (program_id, journal) = deploy_program(
            &mut state.programs,
            DeployProgram {
                owner: Address::ZERO,
                nonce: 1,
                code: valid_code(1),
            },
            Height(1),
        )
        .unwrap();

        assert!(state.programs.contains(&program_id));

        state
            .rollback_state(StateRollbackJournal {
                coin: None,
                program: Some(journal),
                extension: None,
            })
            .unwrap();

        assert_eq!(state, before);
    }
}
#[test]
fn duplicate_program_deployment_is_rejected() {
    let mut registry = ProgramRegistry::default();

    let deploy = DeployProgram {
        owner: Address::ZERO,
        nonce: 1,
        code: valid_code(1),
    };

    let (first_id, _) = deploy_program(&mut registry, deploy.clone(), Height(1)).unwrap();

    let result = deploy_program(&mut registry, deploy, Height(2));

    assert!(matches!(
        result,
        Err(DeployError::Registry(RegistryError::AlreadyExists))
    ));

    assert!(registry.contains(&first_id));
    assert_eq!(registry.len(), 1);
}

#[test]
fn empty_program_is_rejected() {
    let mut registry = ProgramRegistry::default();

    let result = deploy_program(
        &mut registry,
        DeployProgram {
            owner: Address::ZERO,
            nonce: 1,
            code: vec![],
        },
        Height(1),
    );

    assert!(matches!(result, Err(DeployError::EmptyProgram)));
    assert!(registry.is_empty());
}

#[test]
fn oversized_program_is_rejected() {
    let mut registry = ProgramRegistry::default();

    let result = deploy_program(
        &mut registry,
        DeployProgram {
            owner: Address::ZERO,
            nonce: 1,
            code: vec![0; MAX_PROGRAM_CODE_SIZE + 1],
        },
        Height(1),
    );

    assert!(matches!(result, Err(DeployError::ProgramTooLarge)));
    assert!(registry.is_empty());
}

#[test]
fn different_deploys_produce_different_program_ids() {
    let mut registry = ProgramRegistry::default();

    let (first, _) = deploy_program(
        &mut registry,
        DeployProgram {
            owner: Address::ZERO,
            nonce: 1,
            code: valid_code(1),
        },
        Height(1),
    )
    .unwrap();

    let (second, _) = deploy_program(
        &mut registry,
        DeployProgram {
            owner: Address::ZERO,
            nonce: 2,
            code: valid_code(2),
        },
        Height(2),
    )
    .unwrap();

    assert_ne!(first, second);
    assert_eq!(registry.len(), 2);
}
