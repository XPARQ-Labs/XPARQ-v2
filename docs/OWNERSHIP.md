# Program identity and ownership

Coin-share owners, asset-share owners, creators, mint authorities, signer,
payment principal, deployer and miner payout all use ProgramId. There is no
separate wallet address type, encoding or endpoint. The shared primitive lives
in `crypto::ProgramId` and is re-exported by `kernel::program`.

## Signature-policy instances

ProgramId is SHA3-256 with the framed domain `XPARQ_HASH_PROGRAM_ACCOUNT`, over:
`xparq:signature-policy:v1 || scheme_id_u8 || canonical_public_key_bytes`.
Public-key length is validated against the chosen scheme before hashing. Policy,
scheme and full key are bound to the instance identity. Supported schemes remain
ML-DSA44/65/87. Public keys appear in spending proofs, not in UTXO owners.

A witness reveals its public key and signature. The system policy derives its
ProgramId, checks it against the transaction principal, and verifies the signature
against the chain-bound call/payment commitment. No account deployment or
persistent public-key record is needed. Deployed IDs bind the deploying ProgramId,
nonce and code hash under `xparq:program-id:v2` in the artifact hash domain.

## Resolution and execution

Registered IDs execute deployed code and cannot spend through a signature-policy
fallback. Unregistered IDs resolve to the stateless signature policy, which still
requires a witness deriving exactly that ID. Receiving funds does not prove that
anybody possesses the corresponding key. VM calls require deployed code.

Contract requests carry the actual executing ProgramId as monetary actor. Nested
calls carry the parent's ProgramId as caller. Kernel hosts consume only shares
owned by that authenticated actor; call data cannot replace it. Programs enforce
application permissions; kernel enforces conservation, authority, nonce, supply,
resource limits, exact protocol burn and atomic rollback.

The root envelope currently uses a signature-policy instance to pay fees. This
change does not add built-in multisig/timelock applications, arbitrary fee-payer
witnesses or a new authorization entry point. Custom application policies still
use deployed VM code and authenticated caller/state.

## CLI and RPC

Program IDs are exactly 64 hexadecimal characters, with lowercase canonical
output. `wallet program-id` replaces the old identity command. Transfers accept
Program IDs (optionally prefixed with `program:`); miners use `--miner PROGRAM_ID`.

- `/program/account/{program_id}`: paginated UTXOs, balances and program state.
- `/program/balance/{program_id}`: available/reserved coin balance.
- `/explorer/program/{program_id}`: paginated program activity.
- `/program/asset/{asset}/balance/{program_id}`: asset balance and shares.

## Compatibility

Owner remains tag 0 plus 32-byte ProgramId, so canonical coin UTXO weight remains
57 bytes. Chain-spec 6/database schema 14 separate this protocol from older chains.
Wallet files now store `program_id`; restore an older file's mnemonic with the same
scheme into a new file rather than reusing the former identity field. Signing keys
are unchanged; Program IDs use a new policy domain. No production restart, wallet
rewrite or old-ledger migration is performed automatically.
