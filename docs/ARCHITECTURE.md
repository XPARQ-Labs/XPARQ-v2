# Architecture and source ownership

Dependencies flow from `runtime -> extension -> kernel -> crypto`; runtime also
uses kernel directly. Kernel has no dependency on extension.

| Source | Responsibility |
| --- | --- |
| [kernel monetary](../kernel/src/monetary/mod.rs) | Coin/asset types, checked monetary primitives and canonical asset state |
| [kernel program](../kernel/src/program/mod.rs) | Authorization, preparation, system-call contracts, restricted hosts, deployed registry and VM |
| [kernel ledger](../kernel/src/ledger/mod.rs) | Coin UTXOs, canonical state, atomic application, state roots and rollback |
| [extension applications](../extension/src/applications.rs) | Installs the unified monetary application implementation |
| [monetary application](../extension/src/monetary/mod.rs) | Shared monetary program: coin transfer and asset create/mint/transfer/burn through checked hosts |
| [runtime](../runtime/src/main.rs) | Persistence, network, mempool, mining, RPC and application installation |
| [wallet](../wallet/README.md) | Signing, payment selection and user workflows over RPC |

## What is a program?

A system application is compiled Rust application logic in extension. Its wire
contract lives in kernel. A deployed program is validated XPVM bytecode stored
in `LedgerState.programs`. These are different execution implementations behind
ProgramCall dispatch; system applications have not been converted to bytecode.

`SystemProgramId(u32)` selects MONETARY=0 or VM=2 (route 1 is a legacy asset
decoder alias). `ProgramId` is a 32-byte owner-instance identity, including
stateless signature-policy instances and deployed programs. The VM route carries the deployed hash as its
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


XPVM v4 adds typed calldata, authenticated immediate caller/signing context,
conditional control flow, program-local key/value storage, dynamic monetary
requests, and synchronous calls between deployed programs. The caller program
cannot choose the callee's authenticated caller. Calls share fuel and rollback;
active-frame reentrancy is rejected. These primitives allow new application
bytecode without changing the compiled XPQ/asset applications or node binary.
Kernel monetary validation remains authoritative. See [v4](XPVM.md#application-bytecode-v4).

## Program ownership

The ledger has one ownership variant: `Owner::Program(ProgramId)`. A wallet is
an implicit instance of the system signature policy; its key identity determines
its instance ID without deployment or stored account metadata. Deployed instances
use their bytecode policy and cannot spend through the implicit signature path.
See [ownership](OWNERSHIP.md) for resolution, authorization and compatibility.
