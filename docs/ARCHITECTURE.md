# Architecture and source ownership

Dependencies flow from `runtime -> extension -> kernel -> crypto`; runtime also
uses kernel directly. Kernel has no dependency on extension.

| Source | Responsibility |
| --- | --- |
| [kernel monetary](../kernel/src/monetary/mod.rs) | Coin/asset types, checked monetary primitives and canonical asset state |
| [kernel program](../kernel/src/program/mod.rs) | Authorization, preparation, system-call contracts, restricted hosts, deployed registry and VM |
| [kernel ledger](../kernel/src/ledger/mod.rs) | Coin UTXOs, canonical state, atomic application, state roots and rollback |
| [extension applications](../extension/src/applications.rs) | Installs XPQ and asset application implementations |
| [coin application](../extension/src/coin_program/mod.rs) | XPQ transfer execution through CoinHost |
| [asset application](../extension/src/asset_program/mod.rs) | Register/mint/transfer/burn dispatch through AssetHost |
| [runtime](../runtime/src/main.rs) | Persistence, network, mempool, mining, RPC and application installation |
| [wallet](../wallet/README.md) | Signing, payment selection and user workflows over RPC |

## What is a program?

A system application is compiled Rust application logic in extension. Its wire
contract lives in kernel. A deployed program is validated XPVM bytecode stored
in `LedgerState.programs`. These are different execution implementations behind
ProgramCall dispatch; system applications have not been converted to bytecode.

`SystemProgramId(u32)` selects XPQ=0, ASSET=1 or VM=2. `ProgramId` is a 32-byte
hash identifying deployed code. The VM route carries the deployed hash as its
payload. Asset contract IDs identify asset records, not deployed programs.

`LedgerState.extensions.assets` holds canonical asset records and shares in
kernel-owned state. The name `extensions` does not give extension direct mutable
access to ledger state. Coin UTXOs contain XPQ only.

## Execution boundary

Runtime installs `extension::SystemApplications` through `Ledger::with_applications`,
including after snapshot restoration. The executor capability is not serialized.
A bare production kernel fails closed for application execution until an
executor is installed.

Authorization binds the call and XPQ payment to the chain and signer. Preparation
validates payment and previews state growth. Extension receives restricted hosts:
coin effects must match the signed payment; asset execution must match the signed
instruction. Kernel checks ownership, conservation, nonce and supply, stages the
changes and commits only after validation succeeds. Journals restore prior state
on rollback. The same installed applications serve mempool, execution, replay and
RPC asset state-growth quotes.

XPVM runs in kernel and can propose a scalar program-state effect. Version 2
also proposes fixed coin and asset payouts from the executing program’s own
shares. Version 3 adds program-owned asset registration and minting; kernel binds
authority to the executing program, selects mint nonces and checks lifetime supply.
The kernel settles all effects atomically with caller payment and state changes. See [ProgramCall](PROGRAM_CALL.md) and [XPVM](XPVM.md).
