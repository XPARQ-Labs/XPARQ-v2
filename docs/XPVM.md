# XPVM versions 1, 2 and 3

The implemented contract lives in [vm.rs](../kernel/src/program/vm.rs).
Deployed code is bounded to **1 MiB** and validated before registry insertion.

## Header

The 13-byte header uses little-endian integers.

| Byte offset | Field | Accepted value |
| --- | --- | --- |
| 0–3 | Magic | ASCII `XPVM` |
| 4 | Version | 1, 2 or 3 |
| 5–6 | Maximum stack items (u16) | 1–256 |
| 7–8 | Declared memory pages (u16) | 0–16; each page is 65,536 bytes |
| 9–12 | Entry (u32) | 0 |

## Instructions

| Opcode | Instruction | Effect |
| --- | --- | --- |
| 0x00 | NOP | No stack change |
| 0x01 | PUSH_I64 | Read the next 8 bytes as a little-endian signed integer |
| 0x02 | ADD | Pop two values and push their checked sum |
| 0x03 | RETURN | Return the sole stack value; must be the final instruction |
| 0x04 | READ_STATE | Push the program's current i64 state |
| 0x05 | WRITE_STATE | Pop a value and propose the new i64 state |

Each instruction costs 1 fuel. Each declared memory page costs 1 fuel.
READ_STATE costs 2 and WRITE_STATE costs 5. The call ceiling is **65,536 fuel**.
The straight-line program is validated and its required fuel checked before
execution. Arithmetic overflow or insufficient fuel fails execution.

Memory declarations are metered, but v1 has no memory load/store instructions,
branches, coin/asset host calls, filesystem, networking, clock or host randomness.
The interpreter returns a value and optional state effect; kernel applies the
effect to staged registry state and journals it for rollback.

## Deploy and call

`program-deploy` in the [wallet](../wallet/README.md#deployed-programs) signs and
submits deployment. The kernel derives ProgramId from owner, nonce and code hash;
an owner/nonce pair must be unused. A VM call uses SystemProgramId::VM (2), opcode
0, and exactly the deployed 32-byte ProgramId as payload, with an authenticated
XPQ payment. Use `wallet program-call --program-id HEX_ID` to sign and submit a VM call.
The wallet selects XPQ inputs and calculates burn and fees automatically.
See the [persistent counter example](../examples/counter/README.md).

This bytecode returns 7, with stack limit 1 and no memory pages:

```text
5850564d01010000000000000001070000000000000003
```

For a lower-level deployment example, see
[deploy_program.rs](../runtime/examples/deploy_program.rs).

## Contract-owned coin and assets (v2)

Version 2 adds `0x06 COIN_TRANSFER` and `0x07 ASSET_TRANSFER`. Each costs 20
fuel and leaves the stack unchanged. A program may contain at most one of each.
Version 1 rejects these opcodes. Existing v1 bytecode remains executable.

`COIN_TRANSFER` is followed by Borsh `Owner` and a positive little-endian u64
amount in zeno. `ASSET_TRANSFER` adds a 32-byte asset contract ID before the same
recipient/amount encoding; its amount is asset base units. `Owner` is a one-byte
tag (0 address, 1 deployed program), followed by the 32-byte address or ProgramId.
These initial instructions support amounts up to u64::MAX, including for assets
whose ledger accounting uses u128.

The VM proposes transfers; kernel settlement selects only `Owner::Program`
shares belonging to the executing ProgramId. It selects up to 256 inputs per
transfer in canonical share-ID order, stopping when funded, and returns change
to that program. Direct wallet signatures cannot spend program-owned shares.
Contract transfers to another program require that target to be deployed.

Recipient and amount are embedded in code. Calls are public and can trigger the
fixed payout repeatedly while funded. There are no caller checks, branches,
arbitrary call arguments, cross-contract calls, or dynamic withdrawal policies
in this initial transfer instruction set.

Payment, both transfers, and the scalar state effect commit atomically. Failed
execution, insufficient balance, invalid ownership, or output collision commits
nothing. Rollback journals restore all consumed and created shares and program
state. Quotes and consensus use the same settlement preview, accounting for VM
coin UTXO counts, asset state growth, and fuel. The caller pays these costs.

Send funds with `--to program:HEX_PROGRAM_ID` on `sign-spend`, `program-transfer`,
or `program-mint`. Inspect balances using `program-account --program-id HEX_ID`
or `GET /program/account/HEX_ID`. See the [fixed payout example](../examples/vault/README.md).

This refactor changes CoinOutput's Borsh recipient from Address to tagged Owner,
coin state-weight accounting, and the chain specification identity (version 3).
Old serialized transactions and databases must not be reused with this format.
Use a fresh development data directory; there is no database migration here.
All peers must run the matching protocol version.


## Program-owned asset registration and minting (v3)

Version 3 accepts the v1/v2 instructions and adds `0x08 ASSET_REGISTER` (40 fuel)
and `0x09 ASSET_MINT` (20 fuel). Each leaves the stack unchanged. At most one
register and one mint are allowed, alongside the existing one coin and one asset
transfer. Register must precede mint. Versions 1 and 2 reject issuance opcodes.

`ASSET_REGISTER` carries Borsh `RegisterAssetRequest`:

| Field | Encoding |
| --- | --- |
| name | u32 byte length + 1–64 printable ASCII bytes, no outer whitespace |
| max_supply | positive u128 LE base units |
| initial_mint | positive u128 LE base units, no greater than max_supply |
| nonce | u64 LE asset creation nonce |
| skip_if_exists | Borsh bool: 0 or 1 |

The kernel sets both creator and mint authority to `Owner::Program(executing_id)`.
Initial supply is owned by that program. With `skip_if_exists = 0`, an existing
asset rejects the entire call. With `1`, matching registration is skipped, with
no additional initial issuance. Register still incurs its static instruction fuel.

`ASSET_MINT` carries Borsh `MintAssetRequest`:

| Field | Encoding |
| --- | --- |
| asset | tag 0 + 32-byte AssetContract, or tag 1 for the preceding registered asset |
| recipient | Borsh Owner: tag 0 address / 1 deployed program + 32 bytes |
| amount | positive u128 LE base units |

Tag 1 requires a preceding register instruction, including when registration is
skipped. It resolves the program-created asset without embedding an ID that depends
on this program's own code hash. The kernel reads the current mint nonce and uses
its checked successor. Mint succeeds only when this program is the asset's mint
authority. Supply and lifetime `total_minted` must stay within `max_supply`;
burning supply does not reopen the issuance cap. Program recipients must exist.

Settlement applies coin transfer, register, mint, then asset transfer, followed by
caller payment and the scalar state effect within the same staged transaction.
Registration, mint and transfer use distinct share origins. Their rollback journals
preserve the original values even when several operations touch the same asset.
The RPC quote uses that same settlement and charges the caller for state growth
and all instruction fuel. Any failure commits no effects.

Requests are fixed in deployed code, and invocation remains public. These
instructions add asset issuance, while dynamic recipients, sale pricing, payment
inspection and access rules remain future VM capabilities. See the buildable
[asset issuer example](../examples/asset_issuer/README.md).

These additional consensus rules change the chain specification identity to
version 3. All peers must use the matching build. Existing v1/v2 code is accepted
on the new chain, but development databases committed to an older chain identity
require a fresh data directory; no chain migration is included.
