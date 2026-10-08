# Validation and synchronization CPU costs

Block application no longer clones the complete `Chain` to append one block.
Transaction execution remains staged privately. Weight, state root and monetary
checks finish before commit. `Chain::insert_block` completes all fallible checks
before changing its maps or tip, so rejected blocks leave the ledger unchanged.

The private block execution state is now cloned once per block and reused across
its transactions. Prepared calls and deployments use internal staging methods
that do not clone that state again. A failure discards the entire private block
state, including successful earlier transactions. Public single-transaction
methods retain an isolated state copy and commit only on success.

Asset operations capture their accounting record, consumed share IDs and all
possible output IDs in a sparse rollback journal. They no longer clone or diff
the global asset maps. Errors restore the captured entries. The restricted asset
host also rolls back an operation if its application returns an error or unwinds
after mutation. Successful journals retain the historical sorted encoding and
include changed entries only.

Asset state-growth quotes copy only that operation's footprint, including existing
output IDs for collision checks, and use the journal's exact canonical size delta.
Coin-only quotes return before making any extension-state copy. This preserves
the existing burn calculation while avoiding full asset-map serialization.

Synchronization, snapshot restoration, startup body replay and scratch recovery
reuse one Argon2 working buffer per batch through the shared admission checks.
Body replay uses `apply_block_with_pow_memory`; snapshot restoration uses
`validate_block_for_apply_with_memory`. Each block still
gets full structural, linkage, difficulty, PoW, authorization and ledger checks.
Header verification and body application continue to verify PoW independently.
The buffer is allocated lazily in sync/replay/restore and released after the batch;
recovery also reuses its header-validation buffer for body application.

The kernel rejects buffers whose size differs from the consensus parameter.
Argon2 remains 128 MiB, one iteration, one lane. Hash outputs, consensus rules,
chain-spec identity and database encoding are unchanged by these optimizations.

## Reproduce the measurements

```bash
cargo test -p kernel --release --locked --offline --test cpu_validation \
  benchmark_chain_commit_and_pow_allocation -- --ignored --nocapture

cargo test -p kernel --release --lib --locked --offline \
  benchmark_asset_clone_and_sparse_journal -- --ignored --nocapture
```

One release run on an Intel Core i7-8550U produced:

| Workload | Previous allocation/copy path | Optimized path |
| --- | ---: | ---: |
| 128 chain appends, 1,000 historical headers | 6.184 ms | 0.292 ms |
| 128 chain appends, 20,000 historical headers | 166.677 ms | 0.358 ms |
| Eight Argon2 hashes, fresh versus reused buffer | 1,185.100 ms | 646.417 ms |
| 128 asset mints with 20,000 existing shares | 560.771 ms | 0.383 ms |

These are elapsed-time microbenchmarks, not end-to-end synchronization timings
or process CPU measurements. The chain fixture tests bookkeeping only and keeps
64 recent bodies plus genesis; its blocks are not mined. Both chain paths must
produce exactly equal chains. Both hashing paths must produce equal hashes for
the same headers. The hash comparison allocates the reusable buffer before its
timed loop, as a batch would; first-allocation cost still exists. Order, allocator
warmup and machine load can affect these single-run results.

The asset benchmark uses the historical clone-and-global-diff algorithm as its
baseline. It excludes baseline payload decoding; the optimized path includes
decoding. It checks identical final state and journals. It does not measure
signatures, block execution, state roots or synchronization as a whole.

## Verification and remaining costs

`kernel/tests/cpu_validation.rs` checks fresh/reused hash equality after switching
headers, identical ledger bytes through both admission paths, invalid PoW and
wrong-sized buffer rejection, and atomic failures for invalid state roots and
chain insertions. It also compares restored snapshot bytes with the validated
ledger and rejects a historical block with insufficient PoW. Existing workspace
tests cover rollback, recovery, snapshots,
transaction authorization and network synchronization.

Generated asset lifecycle tests additionally compare sparse execution with the
historical clone-and-diff oracle: resulting state, serialized journals, full
serialized size deltas and sparse quotes must agree. A collision on the second
output tests rollback after inputs have been consumed and the first output
created. Host tests cover errors, repeated operations, missing operations and
application panic after mutation.

Ledger state is still cloned once for staged block execution and for public
single-transaction operations; the runtime still clones a
ledger for isolated canonical mutations or branch staging. State-root calculation,
asset supply scans, journal persistence and transaction execution remain costs
to profile separately. General VM previews and rollback still clone state.
This change does not introduce incremental state roots,
an owner index or a replacement database layout.
