//! Universal program-instance identity. Public keys are authorization witnesses,
//! never a separate ledger-owner type.
use crate::{
    AccountSignatureScheme, CodecError, CryptoError, HASH_SIZE, HashDomain, PublicKey,
    canonical_bytes, domain,
};
use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};
use std::{fmt, str::FromStr};

pub const PROGRAM_ID_SIZE: usize = HASH_SIZE;
pub const PROGRAM_ID_STRING_LEN: usize = 2 * PROGRAM_ID_SIZE;

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    BorshSerialize,
    BorshDeserialize,
    Serialize,
    Deserialize,
)]
pub struct ProgramId(pub [u8; PROGRAM_ID_SIZE]);

impl ProgramId {
    pub const ZERO: Self = Self([0; PROGRAM_ID_SIZE]);
    pub const fn from_bytes(bytes: [u8; PROGRAM_ID_SIZE]) -> Self {
        Self(bytes)
    }
    pub const fn as_bytes(&self) -> &[u8; PROGRAM_ID_SIZE] {
        &self.0
    }
    pub const fn into_bytes(self) -> [u8; PROGRAM_ID_SIZE] {
        self.0
    }

    pub fn signature_account(
        scheme: AccountSignatureScheme,
        public_key: &[u8],
    ) -> Result<Self, CryptoError> {
        if public_key.len() != scheme.public_key_size() {
            return Err(CryptoError::InvalidPublicKeyLength);
        }
        let mut material = Vec::with_capacity(26 + public_key.len());
        material.extend_from_slice(b"xparq:signature-policy:v1");
        material.push(scheme.id());
        material.extend_from_slice(public_key);
        Ok(Self(
            domain(HashDomain::ProgramAccount, &material).into_bytes(),
        ))
    }

    /// Immutable deployed instance: deploying principal, nonce and code hash.
    pub fn derive(
        code_owner: Self,
        nonce: u64,
        code_hash: impl AsRef<[u8; HASH_SIZE]>,
    ) -> Result<Self, CodecError> {
        let bytes = canonical_bytes(&(
            b"xparq:program-id:v2",
            code_owner,
            nonce,
            code_hash.as_ref(),
        ))?;
        Ok(Self(domain(HashDomain::XPARQArtifact, &bytes).into_bytes()))
    }
}

impl fmt::Display for ProgramId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&hex::encode(self.0))
    }
}

impl FromStr for ProgramId {
    type Err = CryptoError;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.len() != PROGRAM_ID_STRING_LEN {
            return Err(CryptoError::InvalidProgramIdEncoding);
        }
        let mut bytes = [0; PROGRAM_ID_SIZE];
        hex::decode_to_slice(value, &mut bytes)
            .map_err(|_| CryptoError::InvalidProgramIdEncoding)?;
        Ok(Self(bytes))
    }
}

pub fn program_id_from_public_key(public_key: &PublicKey) -> Result<ProgramId, CryptoError> {
    ProgramId::signature_account(public_key.scheme(), &public_key.bytes)
}
pub fn program_id_from_string(value: &str) -> Result<ProgramId, CryptoError> {
    value.parse()
}
pub fn program_id_to_string(id: &ProgramId) -> String {
    id.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn program_ids_have_one_bounded_hex_encoding() {
        let id = ProgramId([0xab; PROGRAM_ID_SIZE]);
        assert_eq!(id.to_string(), "ab".repeat(32));
        assert_eq!(id.to_string().parse(), Ok(id));
        assert_eq!("AB".repeat(32).parse(), Ok(id));
        for invalid in [
            "a".repeat(63),
            "a".repeat(65),
            "z".repeat(64),
            "0".repeat(45),
        ] {
            assert!(invalid.parse::<ProgramId>().is_err());
        }
    }
    #[test]
    fn signature_policy_identity_binds_scheme_full_key_and_policy_domain() {
        let mut ids = Vec::new();
        for scheme in AccountSignatureScheme::ALL {
            let bytes = vec![0xa5; scheme.public_key_size()];
            let id = ProgramId::signature_account(scheme, &bytes).unwrap();
            // Independent Python hashlib SHA3-256 fixtures for the framed preimage.
            let expected = match scheme.id() {
                1 => "101d5baaa1d0eab6285e7854e79f14e05cf8026549be53f8a8404ab865c32152",
                2 => "db947f9ed968673db36cd796d95a18da47c1facd5f059eaae7c2e67a0a61d9a7",
                3 => "659e13ff0dcb80d7302fd0461e7d03cd954483273cfde3feadf3836e1a305b8f",
                _ => unreachable!(),
            };
            assert_eq!(id.to_string(), expected);
            assert_eq!(
                program_id_from_public_key(&PublicKey {
                    account: scheme,
                    bytes: bytes.clone()
                }),
                Ok(id)
            );
            let mut changed = bytes.clone();
            *changed.last_mut().unwrap() ^= 1;
            assert_ne!(id, ProgramId::signature_account(scheme, &changed).unwrap());
            assert!(ProgramId::signature_account(scheme, &bytes[..bytes.len() - 1]).is_err());
            ids.push(id);
        }
        assert_eq!(
            ids.iter().collect::<std::collections::BTreeSet<_>>().len(),
            ids.len()
        );
    }
}
