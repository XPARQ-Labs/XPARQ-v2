use super::*;
use super::{config::*, gossip::*, state::*};
use kernel::operation::{AuthorizedDeployProgram, BlockOperation};

pub(super) fn submit_deploy(path: Option<&str>, encoded: &str) -> Result<(), String> {
    let database = database_path(path);
    let bytes = hex::decode(encoded).map_err(|error| format!("invalid deploy hex: {error}"))?;
    let deploy: AuthorizedDeployProgram =
        canonical_decode(&bytes).map_err(|error| format!("invalid deploy: {error}"))?;
    let id = insert_pending_operation(
        &database,
        BlockOperation::DeployProgram(Box::new(deploy)),
        false,
    )?;
    println!("accepted deploy_operation={}", hex::encode(id));
    Ok(())
}

pub(super) fn insert_pending_operation(
    database: &Path,
    operation: BlockOperation,
    duplicate_is_ok: bool,
) -> Result<[u8; 32], String> {
    let bytes = canonical_bytes(&operation).map_err(|error| error.to_string())?;
    if bytes.len() > kernel::block::MAX_OPERATION_SIZE {
        return Err("operation exceeds block size limit".into());
    }
    operation
        .validate_structure()
        .map_err(|error| format!("invalid operation: {error:?}"))?;
    let _mutation = state_mutation_lock()?
        .lock()
        .map_err(|_| "state mutation lock is poisoned")?;
    let id = operation
        .id()
        .map_err(|error| error.to_string())?
        .into_bytes();
    let ledger = load_or_initialize(database)?;
    let mut pending = read_pending_operations(database)?;
    if pending.iter().any(|existing| {
        existing
            .id()
            .ok()
            .is_some_and(|hash| hash.into_bytes() == id)
    }) {
        return if duplicate_is_ok {
            Ok(id)
        } else {
            Err("operation is already in mempool".into())
        };
    }
    pending.push(operation);
    validate_pending_operations(&ledger, &pending)?;
    write_pending_operations(database, &pending)?;
    notify_gossip();
    Ok(id)
}

fn apply_pending_operation(
    state: &mut kernel::ledger::LedgerState,
    operation: &BlockOperation,
    chain: kernel::common::ChainContext,
    height: Height,
) -> Result<(), String> {
    match operation {
        BlockOperation::ProgramCall(call) => {
            state
                .apply_program_call_with_applications(
                    (**call).clone(),
                    ProgramId::ZERO,
                    chain,
                    height.0,
                    &extension::SystemApplications,
                )
                .map_err(|error| format!("invalid program call: {error}"))?;
        }
        BlockOperation::DeployProgram(deploy) => {
            state
                .apply_deploy_with_applications(
                    (**deploy).clone(),
                    ProgramId::ZERO,
                    chain,
                    height,
                    &extension::SystemApplications,
                )
                .map_err(|error| format!("invalid deploy: {error}"))?;
        }
    }
    Ok(())
}

fn pending_relay_size(operation: &BlockOperation) -> Result<usize, String> {
    match operation {
        BlockOperation::ProgramCall(call) => {
            let legacy = AuthorizedProgramEnvelope::Program(call.clone());
            let size = canonical_bytes(&legacy)
                .map_err(|error| error.to_string())?
                .len();
            if size > MAX_STORED_TRANSACTION_SIZE {
                return Err("program call exceeds transaction size limit".into());
            }
            Ok(size)
        }
        BlockOperation::DeployProgram(_) => Ok(canonical_bytes(operation)
            .map_err(|error| error.to_string())?
            .len()),
    }
}

pub(super) fn validate_pending_operations(
    ledger: &Ledger,
    pending: &[BlockOperation],
) -> Result<(), String> {
    if pending.len() > MAX_MEMPOOL_TRANSACTIONS {
        return Err("mempool operation count exceeds limit".into());
    }
    let chain = kernel::genesis::chain_context().map_err(|error| error.to_string())?;
    let height = Height(ledger.tip_height().map_or(0, |h| h.0.saturating_add(1)));
    let mut state = ledger.state().clone();
    for operation in pending {
        let bytes = canonical_bytes(operation).map_err(|error| error.to_string())?;
        if bytes.len() > kernel::block::MAX_OPERATION_SIZE {
            return Err("operation cannot fit in a block".into());
        }
        let fee = match operation {
            BlockOperation::ProgramCall(call) => call.payment.charges.miner_fee.as_zeno(),
            BlockOperation::DeployProgram(deploy) => deploy.payment.charges.miner_fee.as_zeno(),
        };
        if fee < minimum_relay_fee(pending_relay_size(operation)?)? {
            return Err("operation relay fee is too low".into());
        }
        apply_pending_operation(&mut state, operation, chain, height)?;
    }
    Ok(())
}

pub(super) fn read_pending_operations(path: &Path) -> Result<Vec<BlockOperation>, String> {
    let bytes = crate::storage::read_mempool(path)?;
    let total = bytes
        .iter()
        .try_fold(0u64, |sum, bytes| sum.checked_add(bytes.len() as u64))
        .ok_or("stored mempool size overflow")?;
    if total > MAX_STORED_MEMPOOL_SIZE || bytes.len() > MAX_MEMPOOL_TRANSACTIONS {
        return Err("stored mempool exceeds limit".into());
    }
    bytes
        .into_iter()
        .map(|bytes| {
            if bytes.is_empty() || bytes.len() > kernel::block::MAX_OPERATION_SIZE {
                return Err("stored operation size is outside allowed range".into());
            }
            canonical_decode(&bytes).map_err(|error| format!("decode pending operation: {error}"))
        })
        .collect()
}

pub(super) fn write_pending_operations(
    path: &Path,
    pending: &[BlockOperation],
) -> Result<(), String> {
    let encoded = encode_pending_operations(pending)?;
    crate::storage::replace_mempool(path, &encoded)
}

pub(super) fn encode_pending_operations(
    pending: &[BlockOperation],
) -> Result<Vec<Vec<u8>>, String> {
    let encoded = pending
        .iter()
        .map(|operation| canonical_bytes(operation).map_err(|error| error.to_string()))
        .collect::<Result<Vec<_>, _>>()?;
    let total = encoded
        .iter()
        .try_fold(0u64, |sum, bytes| sum.checked_add(bytes.len() as u64))
        .ok_or("mempool size overflow")?;
    if total > MAX_STORED_MEMPOOL_SIZE || pending.len() > MAX_MEMPOOL_TRANSACTIONS {
        return Err("mempool exceeds persistence limit".into());
    }
    Ok(encoded)
}

pub(super) fn reconcile_pending_operations(
    ledger: &Ledger,
    pending: Vec<BlockOperation>,
    included: &BTreeSet<[u8; 32]>,
) -> Vec<BlockOperation> {
    let Ok(chain) = kernel::genesis::chain_context() else {
        return Vec::new();
    };
    let height = Height(ledger.tip_height().map_or(0, |h| h.0.saturating_add(1)));
    let mut state = ledger.state().clone();
    let mut retained = Vec::new();
    let mut seen = BTreeSet::new();
    for operation in pending {
        if retained.len() >= MAX_MEMPOOL_TRANSACTIONS {
            break;
        }
        let Ok(id) = operation.id() else {
            continue;
        };
        if included.contains(&id.into_bytes()) || !seen.insert(id.into_bytes()) {
            continue;
        }
        let Ok(bytes) = canonical_bytes(&operation) else {
            continue;
        };
        if bytes.len() > kernel::block::MAX_OPERATION_SIZE {
            continue;
        }
        let fee = match &operation {
            BlockOperation::ProgramCall(call) => call.payment.charges.miner_fee.as_zeno(),
            BlockOperation::DeployProgram(deploy) => deploy.payment.charges.miner_fee.as_zeno(),
        };
        if pending_relay_size(&operation)
            .and_then(minimum_relay_fee)
            .map_or(true, |minimum| fee < minimum)
        {
            continue;
        }
        if apply_pending_operation(&mut state, &operation, chain, height).is_err() {
            continue;
        }
        retained.push(operation);
    }
    retained
}

pub(super) fn submit_transaction(path: Option<&str>, encoded: &str) -> Result<(), String> {
    let database = database_path(path);
    let bytes =
        hex::decode(encoded).map_err(|error| format!("invalid transaction hex: {error}"))?;
    if bytes.is_empty() || bytes.len() > MAX_STORED_TRANSACTION_SIZE {
        return Err("transaction size is outside allowed range".into());
    }
    let transaction: Transaction =
        canonical_decode(&bytes).map_err(|error| format!("invalid transaction: {error}"))?;
    let hash = insert_mempool_transaction(&database, transaction, false)?;
    println!("accepted transaction={}", hex::encode(hash));
    Ok(())
}

pub(super) fn print_mempool(path: Option<&str>) -> Result<(), String> {
    let database = database_path(path);
    let pending = read_pending_operations(&database)?;
    println!("operations: {}", pending.len());
    for operation in pending {
        println!(
            "{}",
            hex::encode(
                operation
                    .id()
                    .map_err(|error| error.to_string())?
                    .into_bytes()
            )
        );
    }
    Ok(())
}

pub(super) fn accept_relayed_transaction(database: &Path, bytes: &[u8]) -> Result<(), String> {
    if bytes.is_empty() || bytes.len() > MAX_STORED_TRANSACTION_SIZE {
        return Err("relayed transaction size is outside allowed range".into());
    }
    let transaction: Transaction =
        canonical_decode(bytes).map_err(|error| format!("invalid relayed transaction: {error}"))?;
    insert_mempool_transaction(database, transaction, true).map(|_| ())
}

pub(super) fn insert_mempool_transaction(
    database: &Path,
    transaction: Transaction,
    duplicate_is_ok: bool,
) -> Result<[u8; 32], String> {
    let encoded = canonical_bytes(&transaction).map_err(|error| error.to_string())?;
    if encoded.len() > MAX_STORED_TRANSACTION_SIZE {
        return Err("transaction exceeds consensus size limit".into());
    }
    transaction
        .validate_structure()
        .map_err(|error| error.to_string())?;
    let hash = transaction.id().map_err(|error| error.to_string())?;
    insert_pending_operation(database, BlockOperation::from(transaction), duplicate_is_ok)?;
    Ok(hash)
}

pub(super) fn reserved_coin_inputs(
    operations: &[BlockOperation],
) -> BTreeSet<kernel::monetary::coin::CoinShare> {
    operations
        .iter()
        .flat_map(|operation| match operation {
            BlockOperation::ProgramCall(call) => call.payment.coin_parts(),
            BlockOperation::DeployProgram(deploy) => deploy.payment.coin_parts(),
        })
        .flat_map(|(inputs, _)| inputs.iter().copied())
        .collect()
}

pub(super) fn minimum_relay_fee(encoded_size: usize) -> Result<u64, String> {
    u64::try_from(encoded_size)
        .ok()
        .and_then(|size| size.checked_mul(MIN_RELAY_FEE_ZENO_PER_BYTE))
        .ok_or("minimum relay fee overflow".into())
}

pub(super) fn read_mempool(path: &Path) -> Result<Vec<Transaction>, String> {
    Ok(read_pending_operations(path)?
        .into_iter()
        .filter_map(|operation| match operation {
            BlockOperation::ProgramCall(call) => Some(AuthorizedProgramEnvelope::Program(call)),
            BlockOperation::DeployProgram(_) => None,
        })
        .collect())
}

/// Legacy program-call view for explorer and transaction RPC consumers.
pub(super) fn block_program_transactions(block: &Block) -> impl Iterator<Item = Transaction> + '_ {
    block.operations().iter().filter_map(|operation| {
        operation
            .as_program_call()
            .map(|call| AuthorizedProgramEnvelope::Program(Box::new(call.clone())))
    })
}

pub(super) fn persist_block_and_mempool(
    path: &Path,
    block: &Block,
    mempool: &[Transaction],
) -> Result<(), String> {
    let included = block
        .operations()
        .iter()
        .map(|operation| {
            operation
                .id()
                .map(|id| id.into_bytes())
                .map_err(|error| error.to_string())
        })
        .collect::<Result<BTreeSet<_>, _>>()?;
    let mut pending = read_pending_operations(path)?
        .into_iter()
        .filter(|operation| matches!(operation, BlockOperation::DeployProgram(_)))
        .filter(|operation| {
            operation
                .id()
                .is_ok_and(|id| !included.contains(&id.into_bytes()))
        })
        .collect::<Vec<_>>();
    pending.extend(mempool.iter().cloned().map(BlockOperation::from));
    persist_block_and_pending(path, block, &pending)
}

pub(super) fn persist_block_and_pending(
    path: &Path,
    block: &Block,
    pending: &[BlockOperation],
) -> Result<(), String> {
    let stored = crate::storage::StoredCanonicalBlock {
        height: block.height().0,

        hash: block.hash().map_err(|error| error.to_string())?.0,

        bytes: block_bytes(block).map_err(|error| error.to_string())?,

        transactions: block_program_transactions(block)
            .map(|transaction| transaction.id().map_err(|error| error.to_string()))
            .collect::<Result<Vec<_>, _>>()?,

        activities: super::index::stored_program_activities(block)?,
    };

    crate::storage::append_block_and_replace_mempool(
        path,
        &stored,
        &encode_pending_operations(pending)?,
    )
}

pub(super) fn persist_chain_and_pending(
    path: &Path,
    ledger: &Ledger,
    pending: &[BlockOperation],
) -> Result<(), String> {
    persist_chain_and_pending_from_store(path, path, ledger, pending)
}

pub(super) fn persist_chain_and_pending_from_store(
    path: &Path,
    source: &Path,
    ledger: &Ledger,
    pending: &[BlockOperation],
) -> Result<(), String> {
    persist_chain_and_pending_from_store_with_snapshot(path, source, ledger, pending, None)
}

pub(super) fn persist_chain_and_pending_from_store_with_snapshot(
    path: &Path,
    source: &Path,
    ledger: &Ledger,
    pending: &[BlockOperation],
    snapshot: Option<(u64, &[u8])>,
) -> Result<(), String> {
    if source != path {
        for (height, _) in ledger.chain.headers() {
            let block = canonical_block(source, ledger, *height)?;
            super::journal::copy_receipt(source, path, ledger, &block)?;
        }
    }
    let blocks = || {
        ledger.chain.headers().map(|(height, _)| {
            let block = canonical_block(source, ledger, *height)?;
            Ok(crate::storage::StoredCanonicalBlock {
                height: block.height().0,
                hash: block.hash().map_err(|error| error.to_string())?.0,
                bytes: block_bytes(&block).map_err(|error| error.to_string())?,
                transactions: block_program_transactions(&block)
                    .map(|transaction| transaction.id().map_err(|error| error.to_string()))
                    .collect::<Result<Vec<_>, String>>()?,
                activities: super::index::stored_program_activities(&block)?,
            })
        })
    };
    let pending = encode_pending_operations(pending)?;
    match snapshot {
        Some(snapshot) => crate::storage::replace_blocks_mempool_and_snapshot_stream(
            path,
            blocks,
            &pending,
            Some(snapshot),
        ),
        None => crate::storage::replace_blocks_and_mempool_stream(path, blocks, &pending),
    }
}
