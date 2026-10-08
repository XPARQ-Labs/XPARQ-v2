use super::*;
use super::{config::*, gossip::*, state::*};
use kernel::operation::{AuthorizedDeployProgram, BlockOperation};

#[derive(Clone)]
struct PendingEntry {
    operation: BlockOperation,
    bytes: Vec<u8>,
    id: [u8; 32],
    relay_size: usize,
}
impl PendingEntry {
    fn new(operation: BlockOperation) -> Result<Self, String> {
        let bytes = canonical_bytes(&operation).map_err(|error| error.to_string())?;
        let id = kernel::crypto::domain(kernel::crypto::HashDomain::Operation, &bytes).into_bytes();
        let relay_size = match &operation {
            BlockOperation::ProgramCall(call) => {
                // Both envelopes use one u8 tag, but their tag values and IDs differ.
                // Count the actual legacy envelope without cloning its payload.
                #[derive(borsh::BorshSerialize)]
                enum LegacyRef<'a> {
                    Program(&'a kernel::program::AuthorizedProgramInvocation),
                }
                usize::try_from(
                    kernel::crypto::canonical_length(&LegacyRef::Program(call))
                        .map_err(|error| error.to_string())?,
                )
                .map_err(|_| "relay size overflow")?
            }
            BlockOperation::DeployProgram(_) => bytes.len(),
        };
        Ok(Self {
            operation,
            bytes,
            id,
            relay_size,
        })
    }
}
#[derive(Clone, Default)]
struct PendingPool {
    entries: Vec<Arc<PendingEntry>>,
    ids: BTreeSet<[u8; 32]>,
    total_bytes: u64,
}
impl PendingPool {
    fn push(&mut self, entry: Arc<PendingEntry>) -> Result<(), String> {
        let total = self
            .total_bytes
            .checked_add(entry.bytes.len() as u64)
            .ok_or("mempool size overflow")?;
        if total > MAX_STORED_MEMPOOL_SIZE || self.entries.len() >= MAX_MEMPOOL_TRANSACTIONS {
            return Err("mempool exceeds persistence limit".into());
        }
        self.total_bytes = total;
        self.ids.insert(entry.id);
        self.entries.push(entry);
        Ok(())
    }
    fn matches(&self, bytes: &[Vec<u8>]) -> bool {
        self.entries.len() == bytes.len()
            && self
                .entries
                .iter()
                .zip(bytes)
                .all(|(entry, raw)| entry.bytes == *raw)
    }
}
static PENDING_CACHE: OnceLock<Mutex<Option<(std::path::PathBuf, Arc<PendingPool>)>>> =
    OnceLock::new();
fn pending_cache() -> &'static Mutex<Option<(std::path::PathBuf, Arc<PendingPool>)>> {
    PENDING_CACHE.get_or_init(|| Mutex::new(None))
}
fn publish_pending(path: &Path, pending: Arc<PendingPool>) {
    *pending_cache()
        .lock()
        .unwrap_or_else(|error| error.into_inner()) = Some((path.to_path_buf(), pending));
}
fn load_pending_pool(path: &Path) -> Result<Arc<PendingPool>, String> {
    // Compare exact committed bytes on every read, including writes outside node helpers.
    let raw = crate::storage::read_mempool(path)?;
    let total = raw
        .iter()
        .try_fold(0u64, |sum, bytes| sum.checked_add(bytes.len() as u64))
        .ok_or("stored mempool size overflow")?;
    if total > MAX_STORED_MEMPOOL_SIZE || raw.len() > MAX_MEMPOOL_TRANSACTIONS {
        return Err("stored mempool exceeds limit".into());
    }
    {
        let cache = pending_cache()
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some((database, pending)) = cache.as_ref() {
            if database == path && pending.matches(&raw) {
                return Ok(Arc::clone(pending));
            }
        }
    }
    let mut pool = PendingPool::default();
    for bytes in &raw {
        if bytes.is_empty() || bytes.len() > kernel::block::MAX_OPERATION_SIZE {
            return Err("stored operation size is outside allowed range".into());
        }
        let operation = canonical_decode(bytes)
            .map_err(|error| format!("decode pending operation: {error}"))?;
        pool.push(Arc::new(PendingEntry::new(operation)?))?;
    }
    let pool = Arc::new(pool);
    // Do not change historical decoding behavior for a noncanonical input encoding.
    if pool.matches(&raw) {
        publish_pending(path, Arc::clone(&pool));
    }
    Ok(pool)
}

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
    let entry = Arc::new(PendingEntry::new(operation)?);
    if entry.bytes.len() > kernel::block::MAX_OPERATION_SIZE {
        return Err("operation exceeds block size limit".into());
    }
    entry
        .operation
        .validate_structure()
        .map_err(|error| format!("invalid operation: {error:?}"))?;
    let _mutation = state_mutation_lock()?
        .lock()
        .map_err(|_| "state mutation lock is poisoned")?;
    let ledger = load_or_initialize(database)?;
    let current = load_pending_pool(database)?;
    let id = entry.id;
    if current.ids.contains(&id) {
        return if duplicate_is_ok {
            Ok(id)
        } else {
            Err("operation is already in mempool".into())
        };
    }
    let mut pending = current.as_ref().clone();
    pending.push(entry)?;
    validate_pending_pool(&ledger, &pending)?;
    let slices: Vec<&[u8]> = pending
        .entries
        .iter()
        .map(|entry| entry.bytes.as_slice())
        .collect();
    crate::storage::replace_mempool_slices(database, &slices)?;
    // Publish only after the complete database transaction commits.
    publish_pending(database, Arc::new(pending));
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
    operations: &[BlockOperation],
) -> Result<(), String> {
    if operations.len() > MAX_MEMPOOL_TRANSACTIONS {
        return Err("mempool operation count exceeds limit".into());
    }
    let cached = pending_cache()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .as_ref()
        .map(|(_, pool)| Arc::clone(pool));
    let mut pending = PendingPool::default();
    for (index, operation) in operations.iter().enumerate() {
        let reusable = cached
            .as_ref()
            .and_then(|pool| pool.entries.get(index))
            .filter(|entry| entry.operation == *operation);
        let entry = if let Some(entry) = reusable {
            Arc::clone(entry)
        } else {
            Arc::new(PendingEntry::new(operation.clone())?)
        };
        pending.push(entry)?;
    }
    validate_pending_pool(ledger, &pending)
}

fn validate_pending_pool(ledger: &Ledger, pending: &PendingPool) -> Result<(), String> {
    let chain = kernel::genesis::chain_context().map_err(|error| error.to_string())?;
    let height = Height(ledger.tip_height().map_or(0, |h| h.0.saturating_add(1)));
    let mut state = ledger.state().clone();
    for entry in &pending.entries {
        if entry.bytes.len() > kernel::block::MAX_OPERATION_SIZE {
            return Err("operation cannot fit in a block".into());
        }
        let fee = match &entry.operation {
            BlockOperation::ProgramCall(call) => call.payment.charges.miner_fee.as_zeno(),
            BlockOperation::DeployProgram(deploy) => deploy.payment.charges.miner_fee.as_zeno(),
        };
        if matches!(entry.operation, BlockOperation::ProgramCall(_))
            && entry.relay_size > MAX_STORED_TRANSACTION_SIZE
        {
            return Err("program call exceeds transaction size limit".into());
        }
        if fee < minimum_relay_fee(entry.relay_size)? {
            return Err("operation relay fee is too low".into());
        }
        apply_pending_operation(&mut state, &entry.operation, chain, height)?;
    }
    Ok(())
}

pub(super) fn read_pending_operations(path: &Path) -> Result<Vec<BlockOperation>, String> {
    Ok(load_pending_pool(path)?
        .entries
        .iter()
        .map(|entry| entry.operation.clone())
        .collect())
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

#[cfg(test)]
mod serialization_benchmarks {
    use super::*;

    fn cache_test_database(label: &str) -> std::path::PathBuf {
        let tick = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("xparq-{label}-{}-{tick}", std::process::id()));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn cache_fixture(nonce: u64) -> BlockOperation {
        use kernel::{
            crypto::{AccountSignatureScheme, SigningSeed, program_id_from_public_key},
            program::{AccountAuthorization, CoinTransition, DeployProgram},
        };
        let seed = SigningSeed::new(AccountSignatureScheme::MlDsa44, Box::new([0x75; 32]));
        let public_key = seed.public_key();
        let owner = program_id_from_public_key(&public_key).unwrap();
        BlockOperation::DeployProgram(Box::new(AuthorizedDeployProgram {
            deploy: DeployProgram {
                owner,
                nonce,
                code: vec![1; 65536].into(),
            },
            payment: CoinTransition::coin(
                owner,
                vec![kernel::monetary::coin::CoinShare::from_bytes([1; 32])],
                vec![kernel::monetary::coin::CoinOutput::new(
                    owner,
                    kernel::monetary::coin::Zeno::ONE,
                )],
            )
            .unwrap(),
            authorization: AccountAuthorization {
                salt: [0; 32],
                public_key,
                signature: seed.sign(b"cache-fixture"),
            },
        }))
    }

    #[test]
    fn cached_call_id_and_relay_size_preserve_distinct_legacy_envelope() {
        let BlockOperation::DeployProgram(signed) = cache_fixture(1) else {
            unreachable!()
        };
        let call = kernel::program::AuthorizedProgramInvocation {
            signer: signed.deploy.owner,
            call: kernel::program::system::script::call::ProgramCall {
                program: kernel::program::system::script::call::SystemProgramId::VM,
                opcode: 0,
                payload: vec![8; 65536],
            },
            payment: signed.payment,
            authorization: signed.authorization,
        };
        let legacy = AuthorizedProgramEnvelope::Program(Box::new(call.clone()));
        let operation = BlockOperation::ProgramCall(Box::new(call));
        let entry = PendingEntry::new(operation.clone()).unwrap();
        assert_eq!(entry.bytes, canonical_bytes(&operation).unwrap());
        assert_eq!(entry.id, operation.id().unwrap().into_bytes());
        assert_eq!(entry.relay_size, canonical_bytes(&legacy).unwrap().len());
        assert_ne!(entry.id, legacy.id().unwrap());
        let mut pool = PendingPool::default();
        pool.push(Arc::new(entry)).unwrap();
        assert!(pool.ids.contains(&operation.id().unwrap().into_bytes()));
        assert!(!pool.ids.contains(&legacy.id().unwrap()));
    }

    #[test]
    fn cached_ids_and_bytes_follow_committed_storage_and_reject_corruption() {
        let path = cache_test_database("pending-cache-source");
        let first = cache_fixture(1);
        let second = cache_fixture(2);
        let a = canonical_bytes(&first).unwrap();
        let b = canonical_bytes(&second).unwrap();
        crate::storage::replace_mempool(&path, &[a.clone()]).unwrap();
        for _ in 0..2 {
            let pool = load_pending_pool(&path).unwrap();
            assert_eq!(pool.entries[0].operation, first);
            assert_eq!(pool.entries[0].bytes, a);
            assert_eq!(pool.entries[0].id, first.id().unwrap().into_bytes());
            assert!(pool.ids.contains(&first.id().unwrap().into_bytes()));
        }
        // Bypass node cache helpers, as can happen during recovery or reorg writes.
        crate::storage::replace_mempool_slices(&path, &[&b, &a]).unwrap();
        let pool = load_pending_pool(&path).unwrap();
        assert_eq!(pool.entries.len(), 2);
        assert_eq!(pool.entries[0].operation, second);
        assert_eq!(pool.entries[1].operation, first);
        assert_eq!(pool.total_bytes, (a.len() + b.len()) as u64);
        crate::storage::replace_mempool(&path, &[vec![255]]).unwrap();
        assert!(load_pending_pool(&path).is_err());
        crate::storage::replace_mempool(&path, &[]).unwrap();
        assert!(load_pending_pool(&path).unwrap().entries.is_empty());
        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn cached_pending_candidates_are_isolated_until_storage_commit() {
        let path = cache_test_database("pending-cache-uncommitted");
        let first = cache_fixture(3);
        let second = cache_fixture(4);
        write_pending_operations(&path, &[first.clone()]).unwrap();
        let original = load_pending_pool(&path).unwrap();
        let mut candidate = original.as_ref().clone();
        candidate
            .push(Arc::new(PendingEntry::new(second.clone()).unwrap()))
            .unwrap();
        assert_eq!(original.entries.len(), 1);
        assert_eq!(candidate.entries.len(), 2);
        // A failed write cannot publish the candidate; committed bytes remain authoritative.
        let invalid = path.join("not-a-directory");
        fs::write(&invalid, b"file").unwrap();
        let slices: Vec<_> = candidate
            .entries
            .iter()
            .map(|entry| entry.bytes.as_slice())
            .collect();
        assert!(crate::storage::replace_mempool_slices(&invalid, &slices).is_err());
        assert_eq!(load_pending_pool(&path).unwrap().entries.len(), 1);
        assert_eq!(read_pending_operations(&path).unwrap(), vec![first]);
        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    #[ignore = "manual retained mempool cache benchmark; use release mode and --nocapture"]
    fn benchmark_retained_pending_cache() {
        use std::{hint::black_box, time::Instant};
        let path = cache_test_database("pending-cache-benchmark");
        let fixture = cache_fixture(0);
        let mut pending = Vec::new();
        for nonce in 0..512 {
            let mut operation = fixture.clone();
            let BlockOperation::DeployProgram(ref mut signed) = operation else {
                unreachable!()
            };
            signed.deploy.nonce = nonce;
            pending.push(operation);
        }
        write_pending_operations(&path, &pending).unwrap();
        let primed = load_pending_pool(&path).unwrap();
        let needle = primed.entries.last().unwrap().id;
        let rounds = 8;
        let start = Instant::now();
        for _ in 0..rounds {
            let raw = crate::storage::read_mempool(&path).unwrap();
            let ops: Vec<BlockOperation> = raw
                .iter()
                .map(|bytes| canonical_decode(bytes).unwrap())
                .collect();
            assert!(
                ops.iter()
                    .any(|operation| operation.id().unwrap().into_bytes() == needle)
            );
            black_box(encode_pending_operations(&ops).unwrap());
        }
        let cold = start.elapsed();
        let start = Instant::now();
        for _ in 0..rounds {
            let pool = load_pending_pool(&path).unwrap();
            assert!(Arc::ptr_eq(&primed, &pool));
            assert!(pool.ids.contains(&needle));
            black_box(
                pool.entries
                    .iter()
                    .map(|entry| entry.bytes.as_slice())
                    .collect::<Vec<_>>(),
            );
        }
        let warm = start.elapsed();
        println!(
            "pending=512 rounds={rounds} raw_read_decode_ids_encode_ms={:.3} raw_read_cache_lookup_borrow_ms={:.3}",
            cold.as_secs_f64() * 1000.0,
            warm.as_secs_f64() * 1000.0
        );
        fs::remove_dir_all(path).unwrap();
    }

    /// Models the serialization/hash work in successful program-call admission.
    /// Does not call apply_pending_operation, verify signatures or access storage.
    #[test]
    #[ignore = "manual mempool serialization benchmark; use release mode and --nocapture"]
    fn benchmark_mempool_repeated_serialization() {
        use kernel::{
            crypto::{AccountSignatureScheme, SigningSeed, program_id_from_public_key},
            monetary::coin::{CoinOutput, CoinShare, Zeno},
            program::system::script::call::{ProgramCall, SystemProgramId},
            program::{AccountAuthorization, AuthorizedProgramInvocation, CoinTransition},
        };
        use std::{
            hint::black_box,
            time::{Duration, Instant},
        };
        let seed = SigningSeed::new(AccountSignatureScheme::MlDsa44, Box::new([0x73; 32]));
        let public_key = seed.public_key();
        let owner = program_id_from_public_key(&public_key).unwrap();
        let authorization = AccountAuthorization {
            salt: [0; 32],
            public_key,
            signature: seed.sign(b"serialization-only-fixture"),
        };
        for payload_bytes in [4096, 65536] {
            for existing in [0usize, 64, 512] {
                let pending: Vec<_> = (0..=existing)
                    .map(|index| {
                        let mut input = [0; 32];
                        input[..8].copy_from_slice(&(index as u64).to_le_bytes());
                        BlockOperation::ProgramCall(Box::new(AuthorizedProgramInvocation {
                            signer: owner,
                            call: ProgramCall {
                                program: SystemProgramId::VM,
                                opcode: 0,
                                payload: vec![7; payload_bytes],
                            },
                            payment: CoinTransition::coin(
                                owner,
                                vec![CoinShare::from_bytes(input)],
                                vec![CoinOutput::new(owner, Zeno::ONE)],
                            )
                            .unwrap(),
                            authorization: authorization.clone(),
                        }))
                    })
                    .collect();
                let new = pending.last().unwrap();
                let BlockOperation::ProgramCall(call) = new else {
                    unreachable!()
                };
                let legacy = AuthorizedProgramEnvelope::Program(call.clone());
                let encoded_bytes = canonical_bytes(new).unwrap().len();
                assert_eq!(canonical_bytes(&legacy).unwrap().len(), encoded_bytes);
                let expected_ids: Vec<_> = pending
                    .iter()
                    .map(|operation| operation.id().unwrap())
                    .collect();
                let legacy_id = legacy.id().unwrap();
                let rounds = 8u64;
                let mut timings = [Duration::ZERO; 5];
                for _ in 0..rounds {
                    let start = Instant::now();
                    assert_eq!(
                        black_box(canonical_bytes(black_box(&legacy)).unwrap()).len(),
                        encoded_bytes
                    );
                    assert_eq!(black_box(black_box(&legacy).id().unwrap()), legacy_id);
                    timings[0] += start.elapsed();
                    let start = Instant::now();
                    assert_eq!(
                        black_box(canonical_bytes(black_box(new)).unwrap()).len(),
                        encoded_bytes
                    );
                    assert_eq!(
                        black_box(black_box(new).id().unwrap()),
                        expected_ids[existing]
                    );
                    for (operation, id) in pending[..existing].iter().zip(&expected_ids) {
                        assert_eq!(black_box(black_box(operation).id().unwrap()), *id);
                    }
                    timings[1] += start.elapsed();
                    let start = Instant::now();
                    for operation in black_box(&pending) {
                        assert_eq!(
                            black_box(canonical_bytes(operation).unwrap()).len(),
                            encoded_bytes
                        );
                    }
                    timings[2] += start.elapsed();
                    let start = Instant::now();
                    for operation in black_box(&pending) {
                        assert_eq!(
                            black_box(pending_relay_size(operation).unwrap()),
                            encoded_bytes
                        );
                    }
                    timings[3] += start.elapsed();
                    let start = Instant::now();
                    let encoded =
                        black_box(encode_pending_operations(black_box(&pending)).unwrap());
                    assert_eq!(encoded.len(), pending.len());
                    assert!(encoded.iter().all(|bytes| bytes.len() == encoded_bytes));
                    timings[4] += start.elapsed();
                }
                let passes = 7 + 4 * existing;
                let total_ms: f64 = timings.iter().map(|t| t.as_secs_f64() * 1000.0).sum();
                println!(
                    "existing={existing} payload_bytes={payload_bytes} operation_bytes={encoded_bytes} rounds={rounds} serializations_per_insert={passes} encoded_bytes_per_insert={} total_ms={total_ms:.3} legacy_size_id_ms={:.3} operation_size_duplicate_ids_ms={:.3} limits_ms={:.3} relay_size_ms={:.3} persistence_encode_ms={:.3}",
                    passes * encoded_bytes,
                    timings[0].as_secs_f64() * 1000.0,
                    timings[1].as_secs_f64() * 1000.0,
                    timings[2].as_secs_f64() * 1000.0,
                    timings[3].as_secs_f64() * 1000.0,
                    timings[4].as_secs_f64() * 1000.0
                );
            }
        }
    }
}
