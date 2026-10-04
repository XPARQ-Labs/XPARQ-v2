//! Active payment, authorization, execution and rollback regression tests.

use super::*;
use crate::program::{AccountAuthorization, system::script::call::ProgramCall};
use crypto::Address;

mod payment_tests {
    use super::*;
    use crate::program::system::{
        asset_program::{asset::Unit, opcode::AssetOpcode, type_::Register},
        script::call::SystemProgramId,
    };
    use crate::{
        consensus::{ProtocolBurn, StateTransitionWeight},
        ledger::{CoinUtxo, LedgerState},
        monetary::coin::{CoinOutput, CoinShare, Zeno},
        program::{AuthorizedProgramEnvelope, AuthorizedProgramInvocation, CoinTransition},
    };
    use crypto::{AccountSignatureScheme, SigningSeed, address_from_public_key};

    #[test]
    fn payment_requires_exact_burn_and_binds_call_without_mutating_state() {
        let seed = SigningSeed::new(AccountSignatureScheme::MlDsa44, Box::new([19; 32]));
        let signer = address_from_public_key(&seed.public_key());
        let chain = ChainContext::new([7; 32]);
        let input = CoinShare::from_bytes([2; crypto::HASH16_SIZE]);
        let call = ProgramCall {
            program: SystemProgramId::ASSET,
            opcode: AssetOpcode::Register as u8,
            payload: borsh::to_vec(&Register {
                name: "STAGED".into(),
                max_supply: Unit::from_units(100),
                initial_mint: Unit::from_units(10),
                mint_authority: signer,
                nonce: 1,
            })
            .unwrap(),
        };
        let mut state = LedgerState::default();
        state
            .utxos
            .insert_coin(
                input,
                CoinUtxo {
                    owner: signer,
                    amount: Zeno::from_zeno(1_000_000),
                },
            )
            .unwrap();
        let original = state.clone();
        let payment = CoinTransition::coin(
            signer,
            vec![input],
            vec![CoinOutput::new(signer, Zeno::from_zeno(1))],
        )
        .unwrap();
        let sign = |payment: CoinTransition| {
            let commitment =
                crate::program::program_invocation_commitment(signer, &call, &payment, chain)
                    .unwrap();
            AuthorizedProgramInvocation {
                signer,
                call: call.clone(),
                payment,
                authorization: AccountAuthorization {
                    public_key: seed.public_key(),
                    signature: seed.sign(commitment.as_bytes()),
                },
            }
        };
        let tx = sign(payment);
        assert!(crate::consensus::validate_program_call(tx.clone(), chain, 0, &state).is_err());
        let mut preview = state.extensions.clone();
        let DecodedProgramCall::Asset(decoded) = decode_program(&call).unwrap() else {
            panic!("expected Asset call")
        };
        preview
            .assets
            .apply(
                &decoded,
                ExecutionContext {
                    signer,
                    commitment: [1; 32],
                },
            )
            .unwrap();
        let growth = (canonical_bytes(&preview).unwrap().len()
            - canonical_bytes(&state.extensions).unwrap().len()) as u64;
        let size = canonical_bytes(&AuthorizedProgramEnvelope::Program(Box::new(tx.clone())))
            .unwrap()
            .len() as u64;
        let burn = ProtocolBurn::for_program_call(
            StateTransitionWeight {
                created_coin_utxos: 1,
                consumed_coin_utxos: 1,
                created_state_weight: growth,
            },
            size,
        )
        .unwrap()
        .total()
        .unwrap();
        let output = Zeno::from_zeno(1_000_000).checked_sub(burn).unwrap();
        let tx = sign(
            CoinTransition::coin(signer, vec![input], vec![CoinOutput::new(signer, output)])
                .unwrap(),
        );
        let prepared =
            crate::consensus::validate_program_call(tx.clone(), chain, 0, &state).unwrap();
        assert_eq!(prepared.required_burn, burn);
        assert_eq!(prepared.created_state_weight, growth);
        assert_eq!(state, original);
        let fee = Zeno::from_zeno(7);
        let fee_burn = ProtocolBurn::for_program_call(
            StateTransitionWeight {
                created_coin_utxos: 2,
                consumed_coin_utxos: 1,
                created_state_weight: growth,
            },
            size,
        )
        .unwrap()
        .total()
        .unwrap();
        let change = Zeno::from_zeno(1_000_000)
            .checked_sub(fee_burn)
            .unwrap()
            .checked_sub(fee)
            .unwrap();
        let payment = CoinTransition::coin_with_charges(
            signer,
            vec![input],
            vec![CoinOutput::new(signer, change)],
            crate::program::CoinCharges::new(fee),
        )
        .unwrap();
        assert_eq!(
            crate::consensus::validate_program_call(sign(payment), chain, 0, &state)
                .unwrap()
                .required_burn,
            fee_burn
        );
        let underpaid = CoinTransition::coin(
            signer,
            vec![input],
            vec![CoinOutput::new(
                signer,
                output.checked_add(Zeno::from_zeno(1)).unwrap(),
            )],
        )
        .unwrap();
        assert!(
            crate::consensus::validate_program_call(sign(underpaid), chain, 0, &state).is_err()
        );
        assert_eq!(state, original);
        assert!(matches!(
            crate::consensus::validate_program_call(tx, ChainContext::new([8; 32]), 0, &state),
            Err(crate::consensus::ProgramConsensusError::InvalidAuthorization)
        ));
    }
}

mod xpq_transfer_tests {
    use super::*;
    use crate::{
        consensus::{
            ProtocolBurn, StateTransitionWeight, validate_program_call as validate_consensus_call,
        },
        ledger::{CoinUtxo, LedgerState},
        monetary::coin::{CoinOutput, CoinShare, Zeno},
        program::{
            AccountAuthorization, AuthorizedProgramEnvelope, AuthorizedProgramInvocation,
            CoinCharges, CoinTransition, program_invocation_commitment,
        },
    };
    use crypto::{Signature, SigningSeed, address_from_public_key};

    fn fixture() -> (LedgerState, AuthorizedProgramInvocation, ChainContext) {
        let keys = SigningSeed::new(Signature::MlDsa44, Box::new([91; 32]));
        let owner = address_from_public_key(&keys.public_key());
        let input = CoinShare::from_bytes([92; crypto::HASH16_SIZE]);
        let amount = 1_000_000;
        let mut state = LedgerState::default();
        state.coin.total_mined = Zeno::from_zeno(amount);
        state
            .utxos
            .insert_coin(
                input,
                CoinUtxo {
                    owner,
                    amount: Zeno::from_zeno(amount),
                },
            )
            .unwrap();
        let chain = ChainContext::new([93; crypto::HASH_SIZE]);
        let call = crate::program::system::coin_program::transfer_call();
        let mut size = 0;
        for _ in 0..8 {
            let fee = (size * 8).max(1);
            let burn = ProtocolBurn::for_program_call(
                StateTransitionWeight {
                    created_coin_utxos: 2,
                    consumed_coin_utxos: 1,
                    created_state_weight: 0,
                },
                size,
            )
            .unwrap()
            .total()
            .unwrap()
            .as_zeno();
            let payment = CoinTransition::coin_with_charges(
                owner,
                vec![input],
                vec![CoinOutput::new(
                    Address([94; crypto::ADDRESS_SIZE]),
                    Zeno::from_zeno(amount - burn - fee),
                )],
                CoinCharges::new(Zeno::from_zeno(fee)),
            )
            .unwrap();
            let commitment = program_invocation_commitment(owner, &call, &payment, chain).unwrap();
            let tx = AuthorizedProgramInvocation {
                signer: owner,
                call: call.clone(),
                payment,
                authorization: AccountAuthorization {
                    public_key: keys.public_key(),
                    signature: keys.sign(commitment.as_bytes()),
                },
            };
            let actual = canonical_bytes(&AuthorizedProgramEnvelope::Program(Box::new(tx.clone())))
                .unwrap()
                .len() as u64;
            if actual == size {
                return (state, tx, chain);
            }
            size = actual;
        }
        panic!("native transaction size did not converge");
    }

    #[test]
    fn xpq_transfer_applies_atomically_and_rolls_back_without_asset_mutation() {
        let (mut state, tx, chain) = fixture();
        let before = state.clone();
        let prepared = validate_consensus_call(tx, chain, 1, &state).unwrap();
        assert_eq!(prepared.created_state_weight, 0);
        let journal = state
            .apply_program_call(
                prepared.invocation.clone(),
                Address([95; crypto::ADDRESS_SIZE]),
                chain,
                prepared.height,
            )
            .unwrap();
        assert_eq!(state.extensions, before.extensions);
        assert!(journal.extension.is_none());
        assert_ne!(state.utxos, before.utxos);
        state.rollback_state(journal).unwrap();
        assert_eq!(state, before);
    }

    #[test]
    fn xpq_transfer_binds_method_payment_and_chain_and_rejects_replay() {
        let (mut state, tx, chain) = fixture();
        let mut changed = tx.clone();
        changed.payment.charges.miner_fee = Zeno::from_zeno(1);
        assert!(!changed.verify_authorizations(chain, 1).unwrap());
        assert!(
            !tx.verify_authorizations(ChainContext::new([96; crypto::HASH_SIZE]), 1)
                .unwrap()
        );
        changed = tx.clone();
        changed.call.opcode = 2;
        assert!(changed.validate_structure().is_err());
        changed = tx.clone();
        changed.call.payload.push(0);
        assert!(changed.validate_structure().is_err());
        state
            .apply_program_call(tx.clone(), Address([95; crypto::ADDRESS_SIZE]), chain, 1)
            .unwrap();
        let after = state.clone();
        assert!(
            state
                .apply_program_call(tx, Address([95; crypto::ADDRESS_SIZE]), chain, 1)
                .is_err()
        );
        assert_eq!(state, after);
    }

    #[test]
    fn xpq_transfer_uses_ledger_owner_for_input_authorization() {
        let (mut state, tx, chain) = fixture();
        let input = tx.payment.coin_parts().unwrap().0[0];
        state.utxos.consume_coin(&input).unwrap();
        state
            .utxos
            .insert_coin(
                input,
                CoinUtxo {
                    owner: Address([97; crypto::ADDRESS_SIZE]),
                    amount: Zeno::from_zeno(1_000_000),
                },
            )
            .unwrap();
        let before = state.clone();
        assert!(matches!(
            validate_consensus_call(tx, chain, 1, &state),
            Err(crate::consensus::ProgramConsensusError::RecipientMismatch)
        ));
        assert_eq!(state, before);
    }
}
