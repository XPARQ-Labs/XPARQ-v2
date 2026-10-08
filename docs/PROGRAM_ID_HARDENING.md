# Program ID derivation and authorization

`ProgramId::signature_account(scheme, public_key)` validates the exact public-key
length before hashing. `program_id_from_public_key(&PublicKey)` returns the same
checked identity. Neither function proves key possession: spending additionally
requires a valid signature against the chain-bound transaction commitment.

The SHA3-256 preimage is the framed domain `XPARQ_HASH_PROGRAM_ACCOUNT`, the
little-endian u64 material length, then `xparq:signature-policy:v1`, scheme ID,
and full canonical public-key bytes. Scheme IDs remain 1/2/3 for ML-DSA44/65/87.
The immutable policy domain distinguishes these instances from deployed code.

Program IDs use exactly 64 hexadecimal characters. Parsing checks length before
decoding; output is lowercase. No separate identity checksum, Base56 parser or
wallet address type remains. Receiving at an ID does not establish key possession.

The ledger has `Owner::Program(ProgramId)` only. Registered instances cannot fall
back to signature-policy spending. Kernel input ownership, supply, fees and burn
checks apply after authorization. See [ownership and APIs](OWNERSHIP.md).

Tests cover exact encoding bounds, independent derivation fixtures, scheme/key
binding, wrong-key and cross-chain proofs, contract ownership, monetary conservation,
rollback, wallet workflows and database restart. Chain-spec 6/schema 14 separate
the new identities from old ledgers and files; there is no automatic migration.
