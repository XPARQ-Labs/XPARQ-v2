use std::{collections::BTreeSet, error::Error as StdError, fmt};

use crypto::{Address, canonical_bytes};

use crate::{
    common::ChainContext,
    consensus::{
        BurnError, ProtocolBurn, StateTransitionWeight, created_coin_output_count,
        validate_exact_burn,
    },
    monetary::coin::{CoinOutput, CoinShare, Zeno},
    operation::BlockOperation,
    program::PreparedProgramInvocation,
    program::{AuthorizedProgramInvocation, IntentError, MAX_PROGRAM_INVOCATION_SIZE},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CoinInputState {
    pub amount: Zeno,
    pub owner: Address,
}

pub trait ProgramStateView {
    fn registry(&self) -> Option<&crate::program::ProgramRegistry> {
        None
    }

    fn extension_state(&self) -> Option<&crate::program::system::script::state::ExtensionState> {
        None
    }

    fn coin(&self, id: CoinShare) -> Option<CoinInputState>;
}

/// Validate one authorized program call against the current ledger state.
pub fn validate_program_call(
    transaction: AuthorizedProgramInvocation,
    chain: ChainContext,
    current_height: u64,
    state: &impl ProgramStateView,
) -> Result<PreparedProgramInvocation, ProgramConsensusError> {
    validate_program_call_with_applications(
        transaction,
        chain,
        current_height,
        state,
        crate::program::application::Applications::default().executor(),
    )
}

pub fn validate_program_call_with_applications(
    transaction: AuthorizedProgramInvocation,
    chain: ChainContext,
    current_height: u64,
    state: &impl ProgramStateView,
    applications: &dyn crate::program::application::ApplicationExecutor,
) -> Result<PreparedProgramInvocation, ProgramConsensusError> {
    transaction
        .validate_structure()
        .map_err(ProgramConsensusError::Intent)?;
    let transaction_size =
        canonical_bytes(&BlockOperation::ProgramCall(Box::new(transaction.clone())))
            .map_err(|_| ProgramConsensusError::Encoding)?
            .len();
    if transaction_size > MAX_PROGRAM_INVOCATION_SIZE {
        return Err(ProgramConsensusError::InvocationTooLarge);
    }
    let valid = transaction
        .verify_authorizations(chain, current_height)
        .map_err(ProgramConsensusError::Intent)?;
    if !valid {
        return Err(ProgramConsensusError::InvalidAuthorization);
    }
    let vm_fuel = match crate::program::system::script::execute::decode_program(&transaction.call)
        .map_err(|_| ProgramConsensusError::Intent(IntentError::InvalidAssetCall))?
    {
        crate::program::system::script::execute::DecodedProgramCall::Vm(id) => {
            let registry = state
                .registry()
                .ok_or(ProgramConsensusError::UnknownProgram)?;
            crate::program::vm::execute_registered(
                registry,
                crate::program::ProgramId::from_bytes(id),
                crate::program::vm::MAX_CALL_FUEL,
            )
            .map_err(ProgramConsensusError::Vm)?
            .fuel_used
        }
        _ => 0,
    };
    let created_state_weight = if transaction.call.program
        == crate::program::system::script::call::SystemProgramId::XPQ
        || transaction.call.program == crate::program::system::script::call::SystemProgramId::VM
    {
        0
    } else {
        let extensions = state
            .extension_state()
            .ok_or(ProgramConsensusError::Intent(IntentError::InvalidAssetCall))?;
        crate::program::program_created_state_weight_with_applications(
            &transaction,
            chain,
            extensions,
            applications,
        )?
    };
    let (inputs, outputs) = transaction
        .payment
        .coin_parts()
        .ok_or(ProgramConsensusError::Intent(IntentError::InvalidAssetCall))?;
    let fee = transaction.payment.charges.miner_fee;
    let actual = validate_coin_inputs(inputs, outputs, fee, transaction.signer, state)?;
    let transition = StateTransitionWeight {
        created_coin_utxos: count_coin_outputs(outputs, fee)?,
        consumed_coin_utxos: inputs.len() as u64,
        created_state_weight,
    };
    let required_burn = ProtocolBurn::for_program_call(transition, transaction_size as u64)?
        .total()?
        .checked_add(Zeno::from_zeno(vm_fuel))
        .ok_or(ProgramConsensusError::ZenoOverflow)?;
    validate_exact_burn(actual, required_burn)?;
    Ok(PreparedProgramInvocation {
        invocation: transaction,
        required_burn,
        created_state_weight,
        height: current_height,
    })
}

pub(crate) fn count_coin_outputs(
    outputs: &[CoinOutput],
    miner_fee: Zeno,
) -> Result<u64, ProgramConsensusError> {
    created_coin_output_count(outputs)?
        .checked_add(u64::from(!miner_fee.is_zero()))
        .ok_or(ProgramConsensusError::Burn(BurnError::WeightOverflow))
}

pub(crate) fn validate_coin_inputs(
    inputs: &[CoinShare],
    outputs: &[CoinOutput],
    miner_fee: Zeno,
    signer: Address,
    state: &impl ProgramStateView,
) -> Result<Zeno, ProgramConsensusError> {
    ensure_unique_coin_ids(inputs.iter().copied())?;

    let mut input_total = Zeno::ZERO;

    for id in inputs {
        let input = state.coin(*id).ok_or(ProgramConsensusError::UtxoNotFound)?;

        if input.owner != signer {
            return Err(ProgramConsensusError::RecipientMismatch);
        }

        input_total = input_total
            .checked_add(input.amount)
            .ok_or(ProgramConsensusError::ZenoOverflow)?;
    }

    let output_total = outputs.iter().try_fold(Zeno::ZERO, |sum, output| {
        sum.checked_add(output.amount)
            .ok_or(ProgramConsensusError::ZenoOverflow)
    })?;

    input_total
        .checked_sub(output_total)
        .and_then(|value| value.checked_sub(miner_fee))
        .ok_or(ProgramConsensusError::ValueMismatch)
}

fn ensure_unique_coin_ids(
    ids: impl IntoIterator<Item = CoinShare>,
) -> Result<(), ProgramConsensusError> {
    let mut unique = BTreeSet::new();

    if ids.into_iter().any(|id| !unique.insert(id)) {
        return Err(ProgramConsensusError::Intent(IntentError::DuplicateInput));
    }

    Ok(())
}

#[derive(Debug)]
pub enum ProgramConsensusError {
    UnknownProgram,
    Vm(crate::program::vm::ExecutionError),
    Encoding,
    InvocationTooLarge,
    Intent(IntentError),
    InvalidAuthorization,
    SignatureSchemeInactive,
    UtxoNotFound,
    RecipientMismatch,
    ZenoOverflow,
    ValueMismatch,
    Burn(BurnError),
}

impl fmt::Display for ProgramConsensusError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownProgram => formatter.write_str("deployed program was not found"),
            Self::Vm(error) => write!(formatter, "VM execution failed: {error:?}"),
            Self::Encoding => formatter.write_str("program call encoding failed"),
            Self::InvocationTooLarge => {
                formatter.write_str("program call exceeds consensus size limit")
            }
            Self::Intent(error) => write!(formatter, "invalid program call intent: {error}"),
            Self::InvalidAuthorization => {
                formatter.write_str("program call authorization is invalid")
            }
            Self::SignatureSchemeInactive => {
                formatter.write_str("program call signature scheme is not active at this height")
            }
            Self::UtxoNotFound => formatter.write_str("program call input UTXO was not found"),

            Self::RecipientMismatch => {
                formatter.write_str("program call input is not committed to this signer")
            }
            Self::ZenoOverflow => formatter.write_str("program call amount overflow"),
            Self::ValueMismatch => {
                formatter.write_str("program call outputs exceed canonical input value")
            }
            Self::Burn(error) => write!(formatter, "invalid protocol burn: {error}"),
        }
    }
}

impl StdError for ProgramConsensusError {}

impl From<BurnError> for ProgramConsensusError {
    fn from(error: BurnError) -> Self {
        Self::Burn(error)
    }
}

#[cfg(test)]
mod p3e_authorization_gate_tests {
    use super::*;

    use crypto::{
        AccountSignatureScheme, HASH_SIZE, HASH16_SIZE, SigningSeed, address_from_public_key,
    };

    use crate::{
        monetary::coin::{CoinOutput, Zeno},
        program::{
            AccountAuthorization, AuthorizedProgramInvocation, CoinTransition,
            program_invocation_commitment,
        },
    };

    const TEST_HEIGHT: u64 = 0;

    fn seed(tag: u8) -> SigningSeed {
        SigningSeed::new(AccountSignatureScheme::MlDsa44, Box::new([tag; 32]))
    }

    fn signer(seed: &SigningSeed) -> Address {
        address_from_public_key(&seed.public_key())
    }

    fn chain(tag: u8) -> ChainContext {
        ChainContext::new([tag; HASH_SIZE])
    }

    fn coin_intent(seed: &SigningSeed, input_tag: u8, amount: u64) -> CoinTransition {
        let owner = signer(seed);

        CoinTransition::coin(
            owner,
            vec![CoinShare::from_bytes([input_tag; HASH16_SIZE])],
            vec![CoinOutput::new(owner, Zeno::from_zeno(amount))],
        )
        .expect("valid coin fixture")
    }

    fn authorize_transfer(
        intent: CoinTransition,
        signer_seed: &SigningSeed,
        chain: ChainContext,
    ) -> AuthorizedProgramInvocation {
        let signer = crypto::address_from_public_key(&signer_seed.public_key());
        let call = crate::program::system::coin_program::transfer_call();
        let commitment = program_invocation_commitment(signer, &call, &intent, chain).unwrap();
        AuthorizedProgramInvocation {
            signer,
            call,
            payment: intent,
            authorization: AccountAuthorization {
                public_key: signer_seed.public_key(),
                signature: signer_seed.sign(commitment.as_bytes()),
            },
        }
    }

    #[test]
    fn deployed_vm_call_requires_registry_and_exact_fuel_burn() {
        use crate::program::system::script::call::{ProgramCall, SystemProgramId};
        use crate::{
            common::Height,
            program::{DeployProgram, ProgramRegistry, deploy_program},
        };
        let owner = seed(7);
        let signer = signer(&owner);
        let chain = chain(0x37);
        let mut registry = ProgramRegistry::default();
        let mut code = b"XPVM".to_vec();
        code.extend_from_slice(&[1, 1, 0, 0, 0, 0, 0, 0, 0]);
        code.push(1);
        code.extend_from_slice(&42i64.to_le_bytes());
        code.push(3);
        let (id, _) = deploy_program(
            &mut registry,
            DeployProgram {
                owner: signer,
                nonce: 1,
                code,
            },
            Height(1),
        )
        .unwrap();
        let call = ProgramCall {
            program: SystemProgramId::VM,
            opcode: 0,
            payload: id.as_bytes().to_vec(),
        };
        let input = CoinShare::from_bytes([9; HASH16_SIZE]);
        let input_amount = Zeno::from_zeno(1_000_000);
        struct VmState {
            registry: ProgramRegistry,
            signer: Address,
            input: CoinShare,
            input_amount: Zeno,
        }
        impl ProgramStateView for VmState {
            fn registry(&self) -> Option<&ProgramRegistry> {
                Some(&self.registry)
            }
            fn coin(&self, id: CoinShare) -> Option<CoinInputState> {
                (id == self.input).then_some(CoinInputState {
                    amount: self.input_amount,
                    owner: self.signer,
                })
            }
        }
        let state = VmState {
            registry,
            signer,
            input,
            input_amount,
        };
        let sign = |burn: u64| {
            let payment = CoinTransition::coin_with_charges(
                signer,
                vec![input],
                vec![CoinOutput::new(
                    signer,
                    Zeno::from_zeno(input_amount.as_zeno() - burn - 1),
                )],
                crate::program::CoinCharges::new(Zeno::ONE),
            )
            .unwrap();
            let commitment = program_invocation_commitment(signer, &call, &payment, chain).unwrap();
            AuthorizedProgramInvocation {
                signer,
                call: call.clone(),
                payment,
                authorization: AccountAuthorization {
                    public_key: owner.public_key(),
                    signature: owner.sign(commitment.as_bytes()),
                },
            }
        };
        let provisional = sign(0);
        let size = canonical_bytes(&BlockOperation::ProgramCall(Box::new(provisional)))
            .unwrap()
            .len() as u64;
        let transition = StateTransitionWeight {
            created_coin_utxos: 2,
            consumed_coin_utxos: 1,
            created_state_weight: 0,
        };
        let base = ProtocolBurn::for_program_call(transition, size)
            .unwrap()
            .total()
            .unwrap()
            .as_zeno();
        let fuel = 2;
        let accepted = sign(base + fuel);
        assert_eq!(
            validate_program_call(accepted.clone(), chain, 1, &state)
                .unwrap()
                .required_burn
                .as_zeno(),
            base + fuel
        );
        assert!(matches!(
            validate_program_call(sign(base), chain, 1, &state),
            Err(ProgramConsensusError::Burn(_))
        ));
        let empty = VmState {
            registry: ProgramRegistry::default(),
            ..state
        };
        assert!(matches!(
            validate_program_call(accepted, chain, 1, &empty),
            Err(ProgramConsensusError::Vm(
                crate::program::vm::ExecutionError::UnknownProgram
            ))
        ));
    }

    #[test]
    fn valid_transfer_call_passes_consensus_authorization_gate() {
        let owner = seed(1);
        let chain = chain(0x11);
        let intent = coin_intent(&owner, 1, 10);

        let call = authorize_transfer(intent, &owner, chain);
        struct EmptyState;
        impl ProgramStateView for EmptyState {
            fn coin(&self, _: CoinShare) -> Option<CoinInputState> {
                None
            }
        }
        assert!(matches!(
            validate_program_call(call, chain, TEST_HEIGHT, &EmptyState),
            Err(ProgramConsensusError::UtxoNotFound)
        ));
    }

    #[test]
    fn cross_chain_transaction_is_rejected_before_state_validation() {
        let owner = seed(2);
        let chain_a = chain(0x21);
        let chain_b = chain(0x22);
        let intent = coin_intent(&owner, 2, 10);

        let call = authorize_transfer(intent, &owner, chain_a);
        struct EmptyState;
        impl ProgramStateView for EmptyState {
            fn coin(&self, _: CoinShare) -> Option<CoinInputState> {
                None
            }
        }

        assert!(matches!(
            validate_program_call(call, chain_b, TEST_HEIGHT, &EmptyState),
            Err(ProgramConsensusError::InvalidAuthorization)
        ));
    }
}
