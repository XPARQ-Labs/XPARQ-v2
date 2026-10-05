# Checked address derivation

Address derivation validates the ML-DSA public-key length before constructing hash
material. `Address::derive(AccountSignatureScheme, &[u8])` and
`address_from_public_key(&PublicKey)` return `Result<Address, CryptoError>`.

| Account scheme | Frozen ID | Public-key bytes |
| --- | --- | --- |
| ML-DSA-44 | 1 | 1,312 |
| ML-DSA-65 | 2 | 1,952 |
| ML-DSA-87 | 3 | 2,592 |

`AccountSignatureScheme::id()` supplies these explicit identifiers, `ALL` lists
the account schemes, and `TryFrom<u8>` rejects unsupported identifiers. Existing
explicit enum discriminants and Borsh `use_discriminant` are retained. The one-byte
scheme wire format remains 1/2/3. Address derivation does not enforce height-based
scheme activation; consensus authorization still applies that policy.

## Compatibility

Valid address payloads use the existing preimage exactly:

```text
SHA3-256(
    b"XPARQ_HASH_ADDRESS"
    || u64_le(1 + public_key_length)
    || scheme_id
    || public_key_bytes
)
```

The length is the existing generic hash-domain framing, not a new serialized key
length inserted into the address payload. Removing it would change valid addresses.
No hash-domain tag, key generation, ownership rule, address payload size, checksum
or Base56 format is changed. Human-readable addresses remain 45-character Base56.
Wallet seed tagging uses the same explicit ID bytes as before.

Three payload golden vectors use fixed literal repeated-byte fixtures and expected
SHA3-256 values independently computed with Python `hashlib`. They freeze the
existing derivation for all account schemes; they are not ownership proofs.
Tests also freeze Borsh scheme bytes, reject all unsupported IDs, reject short,
long and empty keys, and reject a key length belonging to another ML-DSA scheme.

## Rust API migration

The free function previously returned `Address`; it now requires error handling:

```rust
let address = crypto::address_from_public_key(&public_key)?;
let same_address = crypto::Address::derive(public_key.scheme(), &public_key.bytes)?;
```

Applications returning a string error can use `.map_err(|error| error.to_string())?`.
Wallet creation and the deployment example propagate errors. Kernel authorization
fails closed when derivation fails, without panicking or treating an error as a
zero address. Test fixtures deriving from generated keys explicitly assert success.
There is no unchecked public derivation compatibility wrapper.

`InvalidPublicKeyLength` identifies a scheme/length mismatch and
`InvalidAccountScheme` identifies an unsupported numeric account ID. A successful
derivation only confirms structural length and computes an identifier. Actual
ownership requires signature verification in the kernel. Both public key and
signature wire decoders retain their existing length checks.

## Verification

See [verification results](audit/address-verification.txt). Existing mainnet
chain-identity fixtures were not rewritten; the focused kernel library regression
excludes the two previously failing identity/vector tests and retains one ignored
test. This hardening does not fix those independent fixtures.

## Coin ownership across schemes

A coin owner commits to both the account scheme ID and its public-key bytes.
Authorization first requires matching public-key and signature schemes, checked
address derivation equal to the transaction signer, and a valid signature over
its chain-bound commitment. Coin input validation then requires each ledger
owner to equal that signer. Changing transaction fields cannot rewrite a stored
coin owner. Failed application leaves the ledger unchanged.

Regression tests cover all six ordered pairs of distinct ML-DSA account schemes.
Each substituted signature is cryptographically valid, but spending the original
owner's coin fails with `InvalidAuthorization`. Replacing the signer and signing
again passes signature authorization but fails with `RecipientMismatch` against
the ledger. Both attempts leave state unchanged. A positive transfer test consumes
an ML-DSA-44-owned input and creates an ML-DSA-65-owned output; its new ownership
commitment accepts the recipient's proof and rejects the previous owner's proof.

Transferring to a different scheme is permitted when authorized by the current
owner. Spending an existing coin under a different scheme is prohibited. These
checks already exist in the kernel; the regression coverage does not change the
consensus hash or address format. Account schemes currently support only the three
ML-DSA variants above. Falcon is not an enabled account scheme, and numeric ID 4
must not be assigned to it without a protocol change: the general signature
registry already reserves that ID for SQIsign Level 5.
