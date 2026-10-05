# XPVM version 1

The implemented contract lives in [vm.rs](../kernel/src/program/vm.rs).
Deployed code is bounded to **1 MiB** and validated before registry insertion.

## Header

The 13-byte header uses little-endian integers.

| Byte offset | Field | Accepted value |
| --- | --- | --- |
| 0–3 | Magic | ASCII `XPVM` |
| 4 | Version | 1 |
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
READ_STATE adds 2 and WRITE_STATE adds 5. The call ceiling is **65,536 fuel**.
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
