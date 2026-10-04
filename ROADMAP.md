# XPARQ VM Roadmap

This roadmap lists unfinished work only. Remove an item once its acceptance
criteria pass. Implemented behavior is documented in
[Architecture](docs/ARCHITECTURE.md), [XPVM v1](docs/XPVM.md), and
[ProgramCall integration](docs/PROGRAM_CALL.md).

## Verification gate

### Reconcile chain-spec identity fixtures

**Status: open.** Three existing tests still expect an older chain-spec hash:

- `bounded_work_rules_have_frozen_mainnet_chain_spec_identity`
- `frozen_phase4_vectors_match_execution`
- `mainnet_genesis_and_chain_spec_match_the_current_structure`

Review the rules and formats committed by the current hash, confirm the intended
reset baseline, and update the affected fixtures consistently. Keep chain-spec
version **1** as requested; the version number and identity hash are separate.

**Acceptance:** All three tests pass against the reviewed identity, execution
vectors retain their intended state and accounting results, and the complete
workspace test suite passes.

## Planned VM development

The following items extend the current v1 contract. Their wire formats and
activation rules need a design decision before implementation.

| Work | Status | Acceptance criteria |
| --- | --- | --- |
| Monetary host access from VM bytecode | Not implemented | Define restricted coin/asset instructions and authorization. Kernel must enforce ownership, conservation, supply, effect limits and rollback; failed calls must leave canonical state unchanged. |
| System applications as deployed bytecode | Not implemented; depends on monetary host access | Define how XPQ/asset applications move from compiled extension execution to VM code. Compare execution effects, fees, state roots, replay and rollback against the existing application vectors before activation. |
| VM memory access and control flow | Not implemented | Specify load/store and branch semantics, decoding bounds, stack/memory limits and metering. Verify deterministic execution and bounded termination, including malformed code and fuel exhaustion. |
| Wallet workflow for deployed-program calls | Not implemented | Define call construction and fee/fuel handling, then add CLI and interactive flows with signing, offline output and RPC submission. Verify execution, failure, restart and reorg using deployed code. |
| Execution receipts | Contract not defined | Decide which results and costs are exposed, whether receipts are consensus data or derived RPC data, and how indexing handles restart and orphan removal. Document the format and verify it across execution and replay. |

For security review and release requirements, see the
[Security Hardening Roadmap](docs/SECURITY_ROADMAP.md).
