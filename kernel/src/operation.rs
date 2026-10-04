use borsh::{BorshDeserialize, BorshSerialize};
use crypto::{Hash, HashDomain, canonical_bytes, domain};

use crate::{
    common::ChainContext,
    error::CodecError,
    program::DeployProgram,
    program::{
        AccountAuthorization, AuthorizationCommitment, AuthorizedProgramEnvelope,
        AuthorizedProgramInvocation, CoinTransition, ProgramHash,
    },
};

#[derive(BorshSerialize, BorshDeserialize, Clone, Debug, PartialEq, Eq)]
pub struct AuthorizedDeployProgram {
    pub deploy: DeployProgram,
    pub payment: CoinTransition,
    pub authorization: AccountAuthorization,
}

impl AuthorizedDeployProgram {
    pub fn commitment(&self, chain: ChainContext) -> Result<AuthorizationCommitment, CodecError> {
        let code_hash =
            ProgramHash::derive(&self.deploy.code).map_err(|_| CodecError::EncodeFailed)?;
        let bytes = canonical_bytes(&(
            b"xparq:deploy-program:v1",
            chain.genesis_hash,
            self.deploy.owner,
            self.deploy.nonce,
            code_hash,
            &self.payment,
        ))
        .map_err(|_| CodecError::EncodeFailed)?;
        Ok(AuthorizationCommitment::from_bytes(
            domain(HashDomain::XPARQArtifact, &bytes).into_bytes(),
        ))
    }
}

#[derive(BorshSerialize, BorshDeserialize, Clone, Debug, PartialEq, Eq)]
pub enum BlockOperation {
    DeployProgram(Box<AuthorizedDeployProgram>),
    ProgramCall(Box<AuthorizedProgramInvocation>),
}

impl BlockOperation {
    pub fn id(&self) -> Result<Hash, CodecError> {
        let bytes = canonical_bytes(self).map_err(|_| CodecError::EncodeFailed)?;

        Ok(domain(HashDomain::Operation, &bytes))
    }

    pub fn validate_structure(&self) -> Result<(), OperationError> {
        match self {
            Self::DeployProgram(operation) => operation
                .deploy
                .validate_structure()
                .and_then(|_| {
                    operation
                        .payment
                        .validate()
                        .map_err(|_| crate::program::DeployError::InvalidPayment)
                })
                .map_err(|_| OperationError::InvalidDeploy),

            Self::ProgramCall(operation) => operation
                .validate_structure()
                .map_err(|_| OperationError::InvalidProgramCall),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationError {
    InvalidDeploy,
    InvalidProgramCall,
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::{
        monetary::coin::{CoinOutput, CoinShare, Zeno},
        program::CoinTransition,
    };
    use crypto::{AccountSignatureScheme, SigningSeed, address_from_public_key};

    fn deploy_operation(code: Vec<u8>) -> BlockOperation {
        let seed = SigningSeed::new(AccountSignatureScheme::MlDsa44, Box::new([0x41; 32]));

        let public_key = seed.public_key();
        let owner = address_from_public_key(&public_key);

        BlockOperation::DeployProgram(Box::new(AuthorizedDeployProgram {
            deploy: DeployProgram {
                owner,
                nonce: 1,
                code,
            },
            payment: CoinTransition::coin(
                owner,
                vec![CoinShare::from_bytes([0x11; crypto::HASH16_SIZE])],
                vec![CoinOutput::new(owner, Zeno::ONE)],
            )
            .unwrap(),
            authorization: AccountAuthorization {
                public_key,
                signature: seed.sign(b"xparq-operation-test"),
            },
        }))
    }

    #[test]
    fn operation_id_is_deterministic() {
        let operation = deploy_operation(vec![0x01, 0x02]);

        assert_eq!(operation.id().unwrap(), operation.clone().id().unwrap());
    }

    #[test]
    fn different_deploy_code_produces_different_operation_id() {
        let first = deploy_operation(vec![0x01]);
        let second = deploy_operation(vec![0x02]);

        assert_ne!(first.id().unwrap(), second.id().unwrap());
    }

    #[test]
    fn empty_deploy_operation_is_structurally_invalid() {
        let operation = deploy_operation(vec![]);

        assert!(matches!(
            operation.validate_structure(),
            Err(OperationError::InvalidDeploy)
        ));
    }

    #[test]
    fn valid_deploy_operation_is_structurally_valid() {
        let mut code = b"XPVM".to_vec();
        code.extend_from_slice(&[1, 1, 0, 0, 0, 0, 0, 0, 0]);
        code.push(1);
        code.extend_from_slice(&7u64.to_le_bytes());
        code.push(3);
        let operation = deploy_operation(code);

        assert!(operation.validate_structure().is_ok());
    }
}

impl BlockOperation {
    pub fn as_program_call(&self) -> Option<&AuthorizedProgramInvocation> {
        match self {
            Self::ProgramCall(call) => Some(call),
            Self::DeployProgram(_) => None,
        }
    }

    pub fn into_program_call(self) -> Option<Box<AuthorizedProgramInvocation>> {
        match self {
            Self::ProgramCall(call) => Some(call),
            Self::DeployProgram(_) => None,
        }
    }
}

impl From<AuthorizedProgramEnvelope> for BlockOperation {
    fn from(transaction: AuthorizedProgramEnvelope) -> Self {
        match transaction {
            AuthorizedProgramEnvelope::Program(call) => Self::ProgramCall(call),
        }
    }
}
