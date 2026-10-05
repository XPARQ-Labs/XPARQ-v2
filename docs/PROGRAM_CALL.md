# ProgramCall integration

See [architecture](ARCHITECTURE.md), [XPVM v1](XPVM.md), and
[wallet commands](../wallet/README.md#extension-asset-program).

## Canonical operations and identities

Blocks contain `BlockOperation::ProgramCall` or `BlockOperation::DeployProgram`.
`AuthorizedProgramInvocation` signs a call and its `CoinTransition` together;
`AuthorizedProgramEnvelope` is the canonical transaction envelope used by RPC.
Deploy uses `AuthorizedDeployProgram` and its separate authorization path.
The unused standalone AuthorizedProgramCall API and legacy.rs were removed.
Active preparation helpers live in `kernel::program::preparation`.

| Identity | Meaning |
| --- | --- |
| SystemProgramId(u32) | Dispatcher route: XPQ=0, ASSET=1, VM=2 |
| ProgramId (32-byte hash) | Deployed bytecode registry identity |
| Asset contract ID | Canonical asset identity, separate from the deployed registry |

The dispatcher rename preserves numeric tags and canonical Borsh encoding.
XPQ transfer uses route 0, method 1 and an empty payload; its inputs, outputs and
miner fee come from the signed payment. ASSET dispatches registration, mint,
transfer and burn. VM route 2, opcode 0 receives a deployed hash as payload.
System XPQ/asset applications execute in extension; VM bytecode executes in kernel.

## State ownership and authorization

Kernel owns coin UTXOs and counters, `LedgerState.extensions.assets`, and
`LedgerState.programs`. Asset shares are not coin UTXOs. Extension receives
restricted hosts rather than mutable LedgerState. Runtime installs
`extension::SystemApplications` for preview, commit, replay, mempool and quotes,
and reinstalls it on snapshot load. Executor capabilities are not serialized.

The coin host binds consumption, address outputs, miner fee and burn to the signed
payment. AssetHost binds its one operation to the signed asset instruction.
Kernel checks ownership, mint nonce, conservation and supply. Coin supply obeys
`total_mined - total_burned = sum(live coin UTXOs)`; asset supply is reconciled
against live asset shares independently.

`validate_program_call_with_applications` authenticates the call and payment
against chain and height. Call and payment signer must agree. Protocol burn covers
canonical transaction archival weight and positive state growth; miner fee is
separate. Asset growth is the positive difference in canonical extension-state
bytes. Net created coin UTXOs include an optional miner-fee output. State shrinkage
provides no refund.

## Atomic execution and rollback

`LedgerState::apply_program_call_with_applications` and
`apply_deploy_with_applications` validate and apply on staged state. Payment,
application effects and supply checks must succeed before commit. Coin, asset
and program journals preserve prior values for rollback. Block rollback checks
the expected tip and parent state roots before committing the restored state.
Snapshots contain canonical state and rollback journals, not application executors.

## RPC and wallet

| Endpoint | Canonical request / purpose |
| --- | --- |
| POST /transaction | Signed Borsh AuthorizedProgramEnvelope; submits a ProgramCall |
| POST /program/quote | Signed Borsh AuthorizedProgramEnvelope; quotes asset state growth without admission |
| POST /program/deploy | Signed Borsh AuthorizedDeployProgram, without BlockOperation wrapper |
| POST /program/deploy/quote | Draft Borsh AuthorizedDeployProgram before signing; validates deploy structure and returns commitment/burn |
| GET /program/asset/{asset} | Metadata, supply and mint nonce |
| GET /program/asset/{asset}/balance/{address} | Balance and owned shares |

Program quote verifies authorization but does not validate exact payment burn or
funding availability. It returns `created_state_weight`, `vm_fuel` (zero for non-VM calls), and
canonical `tip_hash`. VM quotes execute read-only against the current registry;
they reject missing programs, invalid code, overflow, and exhausted fuel. VM
payments must include one zeno of burn per fuel unit in addition to existing
state and archival burn.
Deployment quote returns `program_id`, `required_protocol_burn`,
`authorization_commitment`, next-block `height` and `tip_hash`. Quotes can become
stale; submission revalidates against current mempool state. A state-growth quote
is not a VM fuel quote. RPC monetary values use raw units; wallet amount inputs
use 8 decimals.

Asset CLI commands cover register/mint/transfer/burn/consolidate/info/balance.
`program-register` creates an asset; `program-deploy` deploys bytecode. Account
and balance responses expose `program_assets`. Explorer indexes include asset
recipients even when they receive no XPQ. See [OpenAPI](openapi.json) for routes.

## Storage and compatibility

Current chain spec version is **1**, storage schema **10**, and newly written
snapshot version **3**. The snapshot loader supports the legacy v1 representation
only when its decoder and chain/schema checks accept it; v2 is not accepted by
this loader. Snapshots must match chain identity, checksum, canonical log and
state invariants. If no snapshot qualifies, startup replays from genesis.

The reset baseline requires fresh compatible chain storage. Schema-mismatched
databases are rejected, not automatically migrated; legacy asset balances are
not imported. Returning the chain-spec version to 1 does not restore an earlier
protocol, since the chain-spec hash commits to the actual rules and formats.

## Verification status and remaining gates

Recent focused checks passed workspace compilation, formatting and Clippy
(with existing warnings), kernel/application boundary tests, snapshot tests and
Program submission/mining/restart coverage. This is not a claim that the entire
workspace test suite passes.

Three existing chain-spec fixture tests still expect older identity bytes:
`bounded_work_rules_have_frozen_mainnet_chain_spec_identity`,
`frozen_phase4_vectors_match_execution`, and
`mainnet_genesis_and_chain_spec_match_the_current_structure`.
They require deliberate fixture/identity reconciliation. On 3 October 2026, the
three-node Program lifecycle passed gossip, mining, restart, stronger-fork reorg,
orphan-index removal and post-reorg chain checks (426.66 s). Both explicitly
enabled wallet CLI lifecycle tests passed: XPQ spend/consolidation and asset
register/mint/transfer/consolidation/burn, recipient history and restart. The asset
test consolidates before burning, so an exact change-share burn cannot remove
the second share needed for consolidation. Formatting also passed. See
[roadmap](../ROADMAP.md) and [security gates](SECURITY_ROADMAP.md).

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --offline
cargo check --workspace --all-targets --offline
cargo test --workspace --offline
cargo build -p node -p wallet --bins --offline
cargo test -p wallet --test program_e2e --offline -- --ignored --test-threads=1
cargo test -p node --test network_e2e --offline program_lifecycle_gossips_across_three_nodes_and_rolls_back_on_reorg -- --nocapture
```

Node integration checks bind local RPC/P2P sockets. GUI work remains deferred.
