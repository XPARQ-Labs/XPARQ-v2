//! Checked asset monetary operations, canonical state, and rollback journals.

use std::collections::{BTreeMap, BTreeSet};

use borsh::{BorshDeserialize, BorshSerialize};

use crypto::HASH_SIZE;
#[cfg(test)]
use crypto::ProgramId;

use super::asset::{AssetContract, AssetError, AssetShare, Metadata, Share, Unit};

pub use super::asset::AssetRecord;

use crate::common::Owner;

use crate::program::system::asset_program::type_::AssetCall;

/// Kernel-owned asset state with immutable public inspection.

///

/// Raw authorization contexts and rollback are not application capabilities.

///

/// ```compile_fail

/// use kernel::monetary::asset_state::ExecutionContext;

/// ```

///

/// ```compile_fail

/// use kernel::monetary::asset_state::AssetState;

/// let _rollback = AssetState::rollback;

/// ```

#[derive(Debug, Clone, Default, PartialEq, Eq, BorshSerialize, BorshDeserialize)]

pub struct AssetState {
    pub(crate) records: BTreeMap<AssetContract, AssetRecord>,

    pub(crate) shares: BTreeMap<Share, AssetShare>,
}

/// The kernel must authenticate the caller and bind `commitment` to this call.
///
/// `actor` is the ledger principal whose authority is exercised by the call.
/// For a signature-policy action it is `Owner::Program(signer)`; for a VM action
/// it is `Owner::Program(program_id)`. The kernel, not call payload data, must
/// bind the actor to the authenticated execution path.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ExecutionContext {
    pub actor: Owner,
    pub commitment: [u8; HASH_SIZE],
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]

pub struct AssetJournal {
    records: Vec<(AssetContract, Option<AssetRecord>)>,

    shares: Vec<(Share, Option<AssetShare>)>,
}

impl AssetJournal {
    /// Exact canonical map-size delta using original snapshots and final touched entries.
    /// Map length prefixes have fixed width; unchanged entries need no serialization.
    pub(crate) fn canonical_delta(&self, state: &AssetState) -> Result<i128, AssetError> {
        fn size<K: BorshSerialize, V: BorshSerialize>(
            key: &K,
            value: Option<&V>,
        ) -> Result<i128, AssetError> {
            value
                .map(|value| {
                    crypto::canonical_bytes(&(key, value))
                        .map(|v| v.len() as i128)
                        .map_err(|_| AssetError::Encoding)
                })
                .unwrap_or(Ok(0))
        }
        let mut delta = 0i128;
        for (key, previous) in &self.records {
            delta = delta
                .checked_add(size(key, state.records.get(key))? - size(key, previous.as_ref())?)
                .ok_or(AssetError::BalanceOverflow)?;
        }
        for (key, previous) in &self.shares {
            delta = delta
                .checked_add(size(key, state.shares.get(key))? - size(key, previous.as_ref())?)
                .ok_or(AssetError::BalanceOverflow)?;
        }
        Ok(delta)
    }

    pub(crate) fn share_changes(&self) -> impl Iterator<Item = (Share, Option<AssetShare>)> + '_ {
        self.shares.iter().copied()
    }

    /// Compose sequential operations while preserving each key's pre-call value.
    /// A later snapshot must not replace the original rollback value.
    pub(crate) fn merge(self, next: Self) -> Self {
        let mut records: BTreeMap<_, _> = self.records.into_iter().collect();
        for (key, previous) in next.records {
            records.entry(key).or_insert(previous);
        }
        let mut shares: BTreeMap<_, _> = self.shares.into_iter().collect();
        for (key, previous) in next.shares {
            shares.entry(key).or_insert(previous);
        }
        Self {
            records: records.into_iter().collect(),
            shares: shares.into_iter().collect(),
        }
    }
}

impl AssetState {
    /// Read-only asset accounting records. Mutation is restricted to the kernel.

    ///

    /// ```compile_fail

    /// use kernel::monetary::asset_state::AssetState;

    /// let mut state = AssetState::default();

    /// state.records().clear();

    /// ```

    pub fn records(&self) -> &BTreeMap<AssetContract, AssetRecord> {
        &self.records
    }

    /// Read-only live shares; applications must use the bound asset host.

    ///

    /// ```compile_fail

    /// use kernel::monetary::asset_state::AssetState;

    /// let mut state = AssetState::default();

    /// state.shares.clear();

    /// ```

    pub fn shares(&self) -> &BTreeMap<Share, AssetShare> {
        &self.shares
    }

    /// Apply a checked monetary instruction; application dispatch belongs to extension.

    pub(crate) fn apply(
        &mut self,

        call: &AssetCall,

        context: ExecutionContext,
    ) -> Result<AssetJournal, AssetError> {
        use crate::program::system::asset_program::opcode::AssetOpcode;

        let (opcode, payload) = match call {
            AssetCall::Register(value) => (AssetOpcode::Register, borsh::to_vec(value)),

            AssetCall::Mint(value) => (AssetOpcode::Mint, borsh::to_vec(value)),

            AssetCall::Transfer(value) => (AssetOpcode::Transfer, borsh::to_vec(value)),

            AssetCall::Burn(value) => (AssetOpcode::Burn, borsh::to_vec(value)),
        };

        let payload = payload.map_err(|_| AssetError::Encoding)?;

        crate::program::system::asset_program::decode(opcode as u8, &payload)
            .map_err(|_| AssetError::InvalidProgram)?;

        let mut journal = self.snapshot_operation(call, context)?;
        if let Err(error) = self.apply_monetary_operation(call, context) {
            self.rollback(journal);
            return Err(error);
        }
        // Preserve the historical journal encoding: sorted keys, changed values only.
        journal
            .records
            .retain(|(key, previous)| self.records.get(key) != previous.as_ref());
        journal
            .shares
            .retain(|(key, previous)| self.shares.get(key) != previous.as_ref());
        Ok(journal)
    }

    /// Include every input, possible output collision and accounting record before
    /// mutation. No monetary operation may read or write outside this footprint.
    fn snapshot_operation(
        &self,
        call: &AssetCall,
        context: ExecutionContext,
    ) -> Result<AssetJournal, AssetError> {
        let (asset, inputs, outputs) = match call {
            AssetCall::Register(call) => {
                let metadata = Metadata::new(
                    call.name.clone(),
                    call.max_supply,
                    context.actor,
                    call.mint_authority,
                )?;
                (AssetContract::derive(&metadata, call.nonce)?, &[][..], 1)
            }
            AssetCall::Mint(call) => (call.asset, &[][..], 1),
            AssetCall::Transfer(call) => (call.asset, call.inputs.as_slice(), call.outputs.len()),
            AssetCall::Burn(call) => (
                call.asset,
                call.inputs.as_slice(),
                usize::from(!call.output.is_zero()),
            ),
        };
        let mut shares: BTreeSet<_> = inputs.iter().copied().collect();
        for index in 0..outputs {
            shares.insert(Share::derive(
                asset,
                context.commitment,
                u32::try_from(index).map_err(|_| AssetError::InvalidProgram)?,
            ));
        }
        Ok(AssetJournal {
            records: vec![(asset, self.records.get(&asset).cloned())],
            shares: shares
                .into_iter()
                .map(|key| (key, self.shares.get(&key).copied()))
                .collect(),
        })
    }

    /// Build a private quote state containing only entries read or written by this
    /// instruction. Existing output IDs are included so collisions fail identically.
    pub(crate) fn operation_view(
        &self,
        call: &AssetCall,
        context: ExecutionContext,
    ) -> Result<Self, AssetError> {
        let journal = self.snapshot_operation(call, context)?;
        Ok(Self {
            records: journal
                .records
                .into_iter()
                .filter_map(|(key, value)| value.map(|value| (key, value)))
                .collect(),
            shares: journal
                .shares
                .into_iter()
                .filter_map(|(key, value)| value.map(|value| (key, value)))
                .collect(),
        })
    }

    pub(crate) fn rollback(&mut self, journal: AssetJournal) {
        for (key, previous) in journal.records {
            match previous {
                Some(record) => {
                    self.records.insert(key, record);
                }

                None => {
                    self.records.remove(&key);
                }
            }
        }

        for (key, previous) in journal.shares {
            match previous {
                Some(share) => {
                    self.shares.insert(key, share);
                }

                None => {
                    self.shares.remove(&key);
                }
            }
        }
    }

    fn input_total(
        &self,
        asset: AssetContract,
        inputs: &[Share],
        actor: Owner,
    ) -> Result<Unit, AssetError> {
        let mut total = Unit::ZERO;

        for input in inputs {
            let share = self.shares.get(input).ok_or(AssetError::UnknownObject)?;

            if share.asset != asset {
                return Err(AssetError::AssetMismatch);
            }

            if share.owner != actor {
                return Err(AssetError::Unauthorized);
            }

            total = total
                .checked_add(share.amount)
                .ok_or(AssetError::BalanceOverflow)?;
        }

        Ok(total)
    }

    fn insert_share(
        &mut self,

        asset: AssetContract,

        commitment: [u8; HASH_SIZE],

        index: usize,

        amount: Unit,

        owner: Owner,
    ) -> Result<(), AssetError> {
        let index = u32::try_from(index).map_err(|_| AssetError::InvalidProgram)?;

        let id = Share::derive(asset, commitment, index);

        if self.shares.contains_key(&id) {
            return Err(AssetError::ShareAlreadyExists);
        }

        self.shares
            .insert(id, AssetShare::new(asset, amount, owner));

        Ok(())
    }

    fn apply_monetary_operation(
        &mut self,

        call: &AssetCall,

        context: ExecutionContext,
    ) -> Result<(), AssetError> {
        match call {
            AssetCall::Register(call) => {
                let metadata = Metadata::new(
                    call.name.clone(),
                    call.max_supply,
                    context.actor,
                    call.mint_authority,
                )?;

                if call.initial_mint.is_zero() || call.initial_mint > call.max_supply {
                    return Err(AssetError::InvalidAmount);
                }

                let asset = AssetContract::derive(&metadata, call.nonce)?;

                if self.records.contains_key(&asset) {
                    return Err(AssetError::AssetAlreadyExists);
                }

                self.insert_share(
                    asset,
                    context.commitment,
                    0,
                    call.initial_mint,
                    context.actor,
                )?;

                self.records.insert(
                    asset,
                    AssetRecord {
                        metadata,

                        supply: call.initial_mint,

                        total_minted: call.initial_mint,

                        mint_nonce: 0,

                        total_burned: Unit::ZERO,
                    },
                );
            }

            AssetCall::Mint(call) => {
                if call.amount.is_zero() {
                    return Err(AssetError::InvalidAmount);
                }

                let record = self
                    .records
                    .get(&call.asset)
                    .ok_or(AssetError::UnknownAsset)?;

                if record.metadata.mint_authority != context.actor {
                    return Err(AssetError::Unauthorized);
                }

                if call.nonce
                    != record
                        .mint_nonce
                        .checked_add(1)
                        .ok_or(AssetError::InvalidMintNonce)?
                {
                    return Err(AssetError::InvalidMintNonce);
                }

                let supply = record
                    .supply
                    .checked_add(call.amount)
                    .ok_or(AssetError::SupplyOverflow)?;

                if supply > record.metadata.max_supply {
                    return Err(AssetError::SupplyOverflow);
                }

                let total_minted = record
                    .total_minted
                    .checked_add(call.amount)
                    .ok_or(AssetError::SupplyOverflow)?;

                if total_minted > record.metadata.max_supply {
                    return Err(AssetError::SupplyOverflow);
                }

                self.insert_share(
                    call.asset,
                    context.commitment,
                    0,
                    call.amount,
                    call.recipient,
                )?;

                let record = self
                    .records
                    .get_mut(&call.asset)
                    .ok_or(AssetError::UnknownAsset)?;

                record.supply = supply;

                record.total_minted = total_minted;

                record.mint_nonce = call.nonce;
            }

            AssetCall::Transfer(call) => {
                if !self.records.contains_key(&call.asset) {
                    return Err(AssetError::UnknownAsset);
                }

                if call.inputs.is_empty() || call.outputs.is_empty() {
                    return Err(AssetError::InvalidProgram);
                }

                super::asset::ensure_unique_asset_inputs(&call.inputs)?;

                let input_total = self.input_total(call.asset, &call.inputs, context.actor)?;

                let mut output_total = Unit::ZERO;

                for output in &call.outputs {
                    if output.amount.is_zero() {
                        return Err(AssetError::InvalidAmount);
                    }

                    output_total = output_total
                        .checked_add(output.amount)
                        .ok_or(AssetError::BalanceOverflow)?;
                }

                if input_total != output_total {
                    return Err(AssetError::InvalidAmount);
                }

                for input in &call.inputs {
                    self.shares.remove(input);
                }

                for (index, output) in call.outputs.iter().enumerate() {
                    self.insert_share(
                        call.asset,
                        context.commitment,
                        index,
                        output.amount,
                        output.recipient,
                    )?;
                }
            }

            AssetCall::Burn(call) => {
                if call.inputs.is_empty() || call.amount.is_zero() {
                    return Err(AssetError::InvalidAmount);
                }

                super::asset::ensure_unique_asset_inputs(&call.inputs)?;

                let input_total = self.input_total(call.asset, &call.inputs, context.actor)?;

                let expected = call
                    .amount
                    .checked_add(call.output)
                    .ok_or(AssetError::BalanceOverflow)?;

                if input_total != expected {
                    return Err(AssetError::InvalidAmount);
                }

                let record = self
                    .records
                    .get(&call.asset)
                    .ok_or(AssetError::UnknownAsset)?;

                let supply = record
                    .supply
                    .checked_sub(call.amount)
                    .ok_or(AssetError::SupplyOverflow)?;

                let total_burned = record
                    .total_burned
                    .checked_add(call.amount)
                    .ok_or(AssetError::SupplyOverflow)?;

                for input in &call.inputs {
                    self.shares.remove(input);
                }

                if !call.output.is_zero() {
                    self.insert_share(
                        call.asset,
                        context.commitment,
                        0,
                        call.output,
                        context.actor,
                    )?;
                }

                let record = self
                    .records
                    .get_mut(&call.asset)
                    .ok_or(AssetError::UnknownAsset)?;

                record.supply = supply;

                record.total_burned = total_burned;
            }
        }

        Ok(())
    }
}

#[cfg(test)]

mod tests {

    use super::*;

    use crate::program::system::asset_program::{
        asset::AssetOutput,
        type_::{Burn, Mint, Register, Transfer},
    };

    // Historical clone-and-diff implementation, retained only as a test oracle.
    fn legacy_apply(
        state: &mut AssetState,
        call: &AssetCall,
        context: ExecutionContext,
    ) -> Result<AssetJournal, AssetError> {
        let mut next = state.clone();
        next.apply_monetary_operation(call, context)?;
        let record_keys: BTreeSet<_> = state
            .records
            .keys()
            .chain(next.records.keys())
            .copied()
            .collect();
        let share_keys: BTreeSet<_> = state
            .shares
            .keys()
            .chain(next.shares.keys())
            .copied()
            .collect();
        let journal = AssetJournal {
            records: record_keys
                .into_iter()
                .filter(|key| state.records.get(key) != next.records.get(key))
                .map(|key| (key, state.records.get(&key).cloned()))
                .collect(),
            shares: share_keys
                .into_iter()
                .filter(|key| state.shares.get(key) != next.shares.get(key))
                .map(|key| (key, state.shares.get(&key).copied()))
                .collect(),
        };
        *state = next;
        Ok(journal)
    }

    fn assert_sparse_matches_full(state: &AssetState, call: &AssetCall, context: ExecutionContext) {
        let before = borsh::to_vec(state).unwrap().len() as i128;
        let mut legacy = state.clone();
        let old = legacy_apply(&mut legacy, call, context).unwrap();
        let mut sparse = state.clone();
        let journal = sparse.apply(call, context).unwrap();
        assert_eq!(sparse, legacy);
        assert_eq!(
            borsh::to_vec(&journal).unwrap(),
            borsh::to_vec(&old).unwrap()
        );
        let delta = borsh::to_vec(&legacy).unwrap().len() as i128 - before;
        assert_eq!(journal.canonical_delta(&sparse).unwrap(), delta);
        let mut view = state.operation_view(call, context).unwrap();
        let preview = view.apply(call, context).unwrap();
        assert_eq!(preview, journal);
        assert_eq!(preview.canonical_delta(&view).unwrap(), delta);
    }

    #[test]
    fn sparse_transfer_collision_restores_consumed_inputs_and_created_outputs() {
        let owner = Owner::Program(ProgramId([41; HASH_SIZE]));
        let mut state = AssetState::default();
        let register = AssetCall::Register(Register {
            name: "Sparse".into(),
            max_supply: Unit::from_units(100),
            initial_mint: Unit::from_units(10),
            mint_authority: owner,
            nonce: 1,
        });
        let context = ExecutionContext {
            actor: owner,
            commitment: [1; HASH_SIZE],
        };
        assert_sparse_matches_full(&state, &register, context);
        state.apply(&register, context).unwrap();
        let asset = *state.records.keys().next().unwrap();
        let input = *state.shares.keys().next().unwrap();
        let context = ExecutionContext {
            actor: owner,
            commitment: [2; HASH_SIZE],
        };
        let collision = Share::derive(asset, context.commitment, 1);
        state.shares.insert(
            collision,
            AssetShare::new(asset, Unit::from_units(7), owner),
        );
        let record = state.records.get_mut(&asset).unwrap();
        record.supply = Unit::from_units(17);
        record.total_minted = Unit::from_units(17);
        let call = AssetCall::Transfer(Transfer {
            asset,
            inputs: vec![input],
            outputs: vec![
                AssetOutput::new(owner, Unit::from_units(4)),
                AssetOutput::new(owner, Unit::from_units(6)),
            ],
        });
        let before = state.clone();
        assert_eq!(
            state.apply(&call, context),
            Err(AssetError::ShareAlreadyExists)
        );
        assert_eq!(state, before);
        let mut view = state.operation_view(&call, context).unwrap();
        let view_before = view.clone();
        assert_eq!(
            view.apply(&call, context),
            Err(AssetError::ShareAlreadyExists)
        );
        assert_eq!(view, view_before);
    }

    #[test]
    #[ignore = "manual clone microbenchmark; use release mode and --nocapture"]
    fn benchmark_asset_clone_and_sparse_journal() {
        use std::{hint::black_box, time::Instant};
        let owner = Owner::Program(ProgramId([41; HASH_SIZE]));
        let mut base = AssetState::default();
        let context = ExecutionContext {
            actor: owner,
            commitment: [1; HASH_SIZE],
        };
        base.apply(
            &AssetCall::Register(Register {
                name: "Bench".into(),
                max_supply: Unit::from_units(1_000_000),
                initial_mint: Unit::from_units(1),
                mint_authority: owner,
                nonce: 1,
            }),
            context,
        )
        .unwrap();
        let asset = *base.records.keys().next().unwrap();
        for index in 0..20_000 {
            base.shares.insert(
                Share::derive(asset, [7; HASH_SIZE], index),
                AssetShare::new(asset, Unit::from_units(1), owner),
            );
        }
        let record = base.records.get_mut(&asset).unwrap();
        record.supply = Unit::from_units(20_001);
        record.total_minted = Unit::from_units(20_001);
        let calls: Vec<_> = (1..=128u64)
            .map(|nonce| {
                let mut commitment = [0; HASH_SIZE];
                commitment[..8].copy_from_slice(&nonce.to_le_bytes());
                (
                    AssetCall::Mint(Mint {
                        asset,
                        recipient: owner,
                        amount: Unit::from_units(1),
                        nonce,
                    }),
                    ExecutionContext {
                        actor: owner,
                        commitment,
                    },
                )
            })
            .collect();
        let mut old = base.clone();
        let mut sparse = base;
        let start = Instant::now();
        let old_journals: Vec<_> = calls
            .iter()
            .map(|(call, context)| legacy_apply(&mut old, call, *context).unwrap())
            .collect();
        let cloned = start.elapsed();
        let start = Instant::now();
        for ((call, context), expected) in calls.iter().zip(old_journals) {
            let journal = sparse.apply(call, *context).unwrap();
            assert_eq!(journal, expected);
            black_box(&sparse);
        }
        let journaled = start.elapsed();
        assert_eq!(sparse, old);
        println!(
            "shares=20000 operations=128 clone_diff_ms={:.3} sparse_journal_ms={:.3}",
            cloned.as_secs_f64() * 1000.0,
            journaled.as_secs_f64() * 1000.0
        );
    }

    #[test]

    fn lifetime_mint_cap_cannot_be_bypassed_by_burning_and_reminting() {
        let owner = ProgramId([1; crypto::PROGRAM_ID_SIZE]);

        let context = |byte| ExecutionContext {
            actor: Owner::Program(owner),
            commitment: [byte; HASH_SIZE],
        };

        let mut state = AssetState::default();

        state
            .apply(
                &AssetCall::Register(Register {
                    name: "CAP".into(),

                    max_supply: Unit::from_units(100),

                    initial_mint: Unit::from_units(90),

                    mint_authority: Owner::Program(owner),

                    nonce: 1,
                }),
                context(1),
            )
            .unwrap();

        let asset = *state.records.keys().next().unwrap();

        let input = *state.shares.keys().next().unwrap();

        state
            .apply(
                &AssetCall::Burn(Burn {
                    asset,

                    inputs: vec![input],

                    amount: Unit::from_units(80),

                    output: Unit::from_units(10),
                }),
                context(2),
            )
            .unwrap();

        let before = state.clone();

        let result = state.apply(
            &AssetCall::Mint(Mint {
                asset,

                nonce: 1,

                recipient: Owner::Program(owner),

                amount: Unit::from_units(20),
            }),
            context(3),
        );

        assert_eq!(result, Err(AssetError::SupplyOverflow));

        assert_eq!(state, before);
    }

    #[test]

    fn transfer_collision_after_consumption_is_atomic() {
        let owner = ProgramId([1; crypto::PROGRAM_ID_SIZE]);

        let context = |byte| ExecutionContext {
            actor: Owner::Program(owner),
            commitment: [byte; HASH_SIZE],
        };

        let mut state = AssetState::default();

        state
            .apply(
                &AssetCall::Register(Register {
                    name: "COLLISION".into(),

                    max_supply: Unit::from_units(100),

                    initial_mint: Unit::from_units(10),

                    mint_authority: Owner::Program(owner),

                    nonce: 1,
                }),
                context(1),
            )
            .unwrap();

        let asset = *state.records.keys().next().unwrap();

        state
            .apply(
                &AssetCall::Mint(Mint {
                    asset,

                    nonce: 1,

                    recipient: Owner::Program(owner),

                    amount: Unit::from_units(5),
                }),
                context(2),
            )
            .unwrap();

        let before = state.clone();

        let result = state.apply(
            &AssetCall::Transfer(Transfer {
                asset,

                inputs: vec![Share::derive(asset, [1; HASH_SIZE], 0)],

                outputs: vec![AssetOutput::new(
                    Owner::Program(owner),
                    Unit::from_units(10),
                )],
            }),
            context(2),
        );

        assert_eq!(result, Err(AssetError::ShareAlreadyExists));

        assert_eq!(state, before);
    }

    #[test]

    fn mint_amount_and_nonce_overflow_fail_without_mutation() {
        let owner = ProgramId([1; crypto::PROGRAM_ID_SIZE]);

        let context = ExecutionContext {
            actor: Owner::Program(owner),
            commitment: [1; HASH_SIZE],
        };

        let mut state = AssetState::default();

        state
            .apply(
                &AssetCall::Register(Register {
                    name: "OVERFLOW".into(),

                    max_supply: Unit::from_units(u128::MAX),

                    initial_mint: Unit::from_units(u128::MAX),

                    mint_authority: Owner::Program(owner),

                    nonce: 1,
                }),
                context,
            )
            .unwrap();

        let asset = *state.records.keys().next().unwrap();

        let before = state.clone();

        assert_eq!(
            state.apply(
                &AssetCall::Mint(Mint {
                    asset,

                    nonce: 1,

                    recipient: Owner::Program(owner),

                    amount: Unit::from_units(1),
                }),
                ExecutionContext {
                    commitment: [2; HASH_SIZE],

                    ..context
                }
            ),
            Err(AssetError::SupplyOverflow)
        );

        assert_eq!(state, before);

        // Synthetic exhausted counter: the next nonce must never wrap to zero.

        state.records.get_mut(&asset).unwrap().mint_nonce = u64::MAX;

        let before = state.clone();

        assert_eq!(
            state.apply(
                &AssetCall::Mint(Mint {
                    asset,

                    nonce: 0,

                    recipient: Owner::Program(owner),

                    amount: Unit::from_units(1),
                }),
                ExecutionContext {
                    commitment: [2; HASH_SIZE],

                    ..context
                }
            ),
            Err(AssetError::InvalidMintNonce)
        );

        assert_eq!(state, before);
    }

    #[test]

    fn generated_asset_lifecycles_preserve_balances_supply_and_exact_rollback() {
        for seed in 1..=16u64 {
            let owners = [
                ProgramId([1; crypto::PROGRAM_ID_SIZE]),
                ProgramId([2; crypto::PROGRAM_ID_SIZE]),
            ];

            let mut state = AssetState::default();

            let initial = state.clone();

            let registration = state
                .apply(
                    &AssetCall::Register(Register {
                        name: "MODEL".into(),

                        max_supply: Unit::from_units(10_000),

                        initial_mint: Unit::from_units(1_000),

                        mint_authority: Owner::Program(owners[0]),

                        nonce: seed,
                    }),
                    ExecutionContext {
                        actor: Owner::Program(owners[0]),
                        commitment: [0; HASH_SIZE],
                    },
                )
                .unwrap();

            let asset = *state.records.keys().next().unwrap();

            let mut balances = [1_000u64, 0];

            let mut minted = 1_000u64;

            let mut burned = 0u64;

            let mut nonce = 0u64;

            let mut random = seed;

            let mut journals = vec![(initial, registration)];

            for step in 1..=64u64 {
                random ^= random << 13;

                random ^= random >> 7;

                random ^= random << 17;

                let selected = (random & 1) as usize;

                let mut commitment = [0; HASH_SIZE];

                commitment[..8].copy_from_slice(&seed.to_le_bytes());

                commitment[8..16].copy_from_slice(&step.to_le_bytes());

                let inputs = state
                    .shares
                    .iter()
                    .filter(|(_, share)| share.owner == Owner::Program(owners[selected]))
                    .map(|(id, _)| *id)
                    .collect::<Vec<_>>();

                let before = state.clone();

                let (call, signer) = if random % 3 == 0 || balances[selected] == 0 {
                    let amount = random % 20 + 1;

                    nonce += 1;

                    minted += amount;

                    balances[selected] += amount;

                    (
                        AssetCall::Mint(Mint {
                            asset,

                            nonce,

                            recipient: Owner::Program(owners[selected]),

                            amount: Unit::from_units(amount.into()),
                        }),
                        owners[0],
                    )
                } else if random % 3 == 1 {
                    let amount = random % balances[selected] + 1;

                    let change = balances[selected] - amount;

                    let mut outputs = vec![AssetOutput::new(
                        Owner::Program(owners[1 - selected]),
                        Unit::from_units(amount.into()),
                    )];

                    if change > 0 {
                        outputs.push(AssetOutput::new(
                            Owner::Program(owners[selected]),
                            Unit::from_units(change.into()),
                        ));
                    }

                    balances[selected] = change;

                    balances[1 - selected] += amount;

                    (
                        AssetCall::Transfer(Transfer {
                            asset,

                            inputs,

                            outputs,
                        }),
                        owners[selected],
                    )
                } else {
                    let amount = random % balances[selected] + 1;

                    balances[selected] -= amount;

                    burned += amount;

                    (
                        AssetCall::Burn(Burn {
                            asset,

                            inputs,

                            amount: Unit::from_units(amount.into()),

                            output: Unit::from_units(balances[selected].into()),
                        }),
                        owners[selected],
                    )
                };

                assert_sparse_matches_full(
                    &state,
                    &call,
                    ExecutionContext {
                        actor: Owner::Program(signer),
                        commitment,
                    },
                );
                let journal = state
                    .apply(
                        &call,
                        ExecutionContext {
                            actor: Owner::Program(signer),
                            commitment,
                        },
                    )
                    .unwrap();

                for (index, owner) in owners.iter().enumerate() {
                    let actual: u128 = state
                        .shares
                        .values()
                        .filter(|share| share.owner == Owner::Program(*owner))
                        .map(|share| share.amount.as_units())
                        .sum();

                    assert_eq!(actual, u128::from(balances[index]));
                }

                assert_eq!(
                    state.records[&asset].supply.as_units(),
                    u128::from(minted - burned)
                );

                assert_eq!(
                    state.records[&asset].total_minted.as_units(),
                    u128::from(minted)
                );

                assert_eq!(
                    state.records[&asset].total_burned.as_units(),
                    u128::from(burned)
                );

                let mut ledger = crate::ledger::LedgerState::default();

                ledger.extensions.assets = state.clone();

                ledger.validate_supply_invariants().unwrap();

                journals.push((before, journal));
            }

            for (before, journal) in journals.into_iter().rev() {
                let encoded = borsh::to_vec(&journal).unwrap();

                state.rollback(AssetJournal::try_from_slice(&encoded).unwrap());

                assert_eq!(state, before);
            }

            assert_eq!(state, AssetState::default());
        }
    }

    #[test]

    fn lifecycle_authorization_conservation_and_rollback() {
        let owner = ProgramId::from_bytes([1; crypto::PROGRAM_ID_SIZE]);

        let receiver = ProgramId::from_bytes([2; crypto::PROGRAM_ID_SIZE]);

        let context = |signer, byte| ExecutionContext {
            actor: Owner::Program(signer),
            commitment: [byte; HASH_SIZE],
        };

        let mut state = AssetState::default();

        let register = Register {
            name: "TEST".into(),

            max_supply: Unit::from_units(100),

            initial_mint: Unit::from_units(40),

            mint_authority: Owner::Program(owner),

            nonce: 7,
        };

        let register_journal = state
            .apply(&AssetCall::Register(register.clone()), context(owner, 1))
            .unwrap();

        let metadata = Metadata::new(
            register.name,
            register.max_supply,
            Owner::Program(owner),
            Owner::Program(owner),
        )
        .unwrap();

        let asset = AssetContract::derive(&metadata, 7).unwrap();

        let first = Share::derive(asset, [1; HASH_SIZE], 0);

        let mint = AssetCall::Mint(Mint {
            asset,

            nonce: 1,

            recipient: Owner::Program(owner),

            amount: Unit::from_units(20),
        });

        let before = state.clone();

        assert_eq!(
            state.apply(&mint, context(receiver, 2)),
            Err(AssetError::Unauthorized)
        );

        assert_eq!(state, before);

        let mint_journal = state.apply(&mint, context(owner, 2)).unwrap();

        let second = Share::derive(asset, [2; HASH_SIZE], 0);

        let transfer = AssetCall::Transfer(Transfer {
            asset,

            inputs: vec![first, second],

            outputs: vec![AssetOutput::new(
                Owner::Program(receiver),
                Unit::from_units(60),
            )],
        });

        let before = state.clone();

        assert_eq!(
            state.apply(&transfer, context(receiver, 3)),
            Err(AssetError::Unauthorized)
        );

        assert_eq!(state, before);

        let journal = state.apply(&transfer, context(owner, 3)).unwrap();

        let received = Share::derive(asset, [3; HASH_SIZE], 0);

        assert_eq!(
            state.shares.get(&received).unwrap().owner,
            Owner::Program(receiver)
        );

        state.rollback(journal);

        assert_eq!(state, before);

        let transfer_journal = state.apply(&transfer, context(owner, 3)).unwrap();

        let burn = AssetCall::Burn(Burn {
            asset,

            inputs: vec![received],

            amount: Unit::from_units(10),

            output: Unit::from_units(50),
        });

        assert_eq!(
            state.apply(&burn, context(owner, 4)),
            Err(AssetError::Unauthorized)
        );

        let burn_journal = state.apply(&burn, context(receiver, 4)).unwrap();

        assert_eq!(state.records[&asset].supply, Unit::from_units(50));

        assert_eq!(state.records[&asset].total_burned, Unit::from_units(10));

        for journal in [
            burn_journal,
            transfer_journal,
            mint_journal,
            register_journal,
        ] {
            let encoded = borsh::to_vec(&journal).unwrap();

            state.rollback(AssetJournal::try_from_slice(&encoded).unwrap());
        }

        assert_eq!(state, AssetState::default());
    }
}
