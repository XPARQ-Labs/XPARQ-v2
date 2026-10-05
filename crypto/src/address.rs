use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};

use crate::{AccountSignatureScheme, HashDomain, PublicKey, error::CryptoError, hash};

pub const ADDRESS_SIZE: usize = 32;
pub const ADDRESS_ENCODED_SIZE: usize = 45;
pub const ADDRESS_STRING_LEN: usize = ADDRESS_ENCODED_SIZE;

// Human-friendly Base56 alphabet.
// Excludes visually ambiguous characters: O, o, I, i, L, l.
const CHARACTER: &[u8; 56] = b"0123456789ABCDEFGHJKMNPQRSTUVWXYZabcdefghjkmnpqrstuvwxyz";

const TOTAL_CHARACTER: u16 = CHARACTER.len() as u16;

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    BorshSerialize,
    BorshDeserialize,
)]
pub struct Address(pub [u8; ADDRESS_SIZE]);

impl Address {
    pub const ZERO: Self = Self([0; ADDRESS_SIZE]);

    pub const fn from_bytes(bytes: [u8; ADDRESS_SIZE]) -> Self {
        Self(bytes)
    }

    pub const fn as_bytes(&self) -> &[u8; ADDRESS_SIZE] {
        &self.0
    }

    pub const fn into_bytes(self) -> [u8; ADDRESS_SIZE] {
        self.0
    }

    /// Derive using the existing framed hash preimage after checking key length.
    /// Length validation is structural; ownership still requires a valid signature.
    pub fn derive(scheme: AccountSignatureScheme, public_key: &[u8]) -> Result<Self, CryptoError> {
        if public_key.len() != scheme.public_key_size() {
            return Err(CryptoError::InvalidPublicKeyLength);
        }
        let mut material = Vec::with_capacity(1 + public_key.len());
        material.push(scheme.id());
        material.extend_from_slice(public_key);
        let digest = hash::domain(HashDomain::Address, &material);
        Ok(Self::from_bytes(*digest.as_bytes()))
    }
}

/// Reject malformed public keys instead of producing an address from arbitrary bytes.
pub fn address_from_public_key(public_key: &PublicKey) -> Result<Address, CryptoError> {
    Address::derive(public_key.scheme(), &public_key.bytes)
}

pub fn address_to_string(address: &Address) -> String {
    xparq_encode(address.as_bytes())
}

pub fn address_from_string(value: &str) -> Result<Address, CryptoError> {
    if value.len() != ADDRESS_STRING_LEN {
        return Err(CryptoError::InvalidAddressEncoding);
    }

    let bytes = xparq_decode(value)?;
    let address = Address::from_bytes(bytes);

    // Enforce canonical encoding.
    if value != address_to_string(&address) {
        return Err(CryptoError::InvalidAddressEncoding);
    }

    Ok(address)
}

fn xparq_encode(payload: &[u8; ADDRESS_SIZE]) -> String {
    let mut number = *payload;

    let mut encoded = [CHARACTER[0]; ADDRESS_ENCODED_SIZE];

    for position in (0..ADDRESS_ENCODED_SIZE).rev() {
        let mut remainder = 0_u16;

        for byte in &mut number {
            let value = (remainder << 8) | (*byte as u16);

            *byte = (value / TOTAL_CHARACTER) as u8;
            remainder = value % TOTAL_CHARACTER;
        }

        encoded[position] = CHARACTER[remainder as usize];
    }

    debug_assert!(number.iter().all(|byte| *byte == 0));

    String::from_utf8(encoded.to_vec()).expect("XPARQ alphabet must be valid ASCII")
}

fn xparq_decode(encoded: &str) -> Result<[u8; ADDRESS_SIZE], CryptoError> {
    if encoded.len() != ADDRESS_ENCODED_SIZE {
        return Err(CryptoError::InvalidAddressEncoding);
    }

    let mut payload = [0_u8; ADDRESS_SIZE];

    for character in encoded.bytes() {
        let digit = xparq_digit(character).ok_or(CryptoError::InvalidAddressEncoding)?;

        let mut carry = digit as u16;

        // payload = payload * 56 + digit
        for byte in payload.iter_mut().rev() {
            let value = (*byte as u16) * TOTAL_CHARACTER + carry;

            *byte = value as u8;
            carry = value >> 8;
        }

        if carry != 0 {
            return Err(CryptoError::InvalidAddressEncoding);
        }
    }

    Ok(payload)
}

#[inline]
fn xparq_digit(character: u8) -> Option<u8> {
    CHARACTER
        .iter()
        .position(|&candidate| candidate == character)
        .map(|index| index as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scheme_ids_and_borsh_wire_bytes_are_frozen() {
        let schemes = [
            AccountSignatureScheme::MlDsa44,
            AccountSignatureScheme::MlDsa65,
            AccountSignatureScheme::MlDsa87,
        ];
        assert_eq!(AccountSignatureScheme::ALL, schemes);
        for (scheme, id) in schemes.into_iter().zip([1, 2, 3]) {
            assert_eq!(scheme.id(), id);
            assert_eq!(AccountSignatureScheme::try_from(id), Ok(scheme));
            assert_eq!(borsh::to_vec(&scheme).unwrap(), vec![id]);
            assert_eq!(
                borsh::from_slice::<AccountSignatureScheme>(&[id]).unwrap(),
                scheme
            );
        }
        for id in 0..=u8::MAX {
            if !(1..=3).contains(&id) {
                assert_eq!(
                    AccountSignatureScheme::try_from(id),
                    Err(CryptoError::InvalidAccountScheme)
                );
                assert!(borsh::from_slice::<AccountSignatureScheme>(&[id]).is_err());
            }
        }
    }

    #[test]
    fn both_derivation_entry_points_reject_wrong_key_lengths_and_scheme_pairings() {
        for scheme in AccountSignatureScheme::ALL {
            for length in [
                0,
                scheme.public_key_size() - 1,
                scheme.public_key_size() + 1,
            ] {
                let key = PublicKey {
                    account: scheme,
                    bytes: vec![0; length],
                };
                assert_eq!(
                    Address::derive(scheme, &key.bytes),
                    Err(CryptoError::InvalidPublicKeyLength)
                );
                assert_eq!(
                    address_from_public_key(&key),
                    Err(CryptoError::InvalidPublicKeyLength)
                );
            }
            for other in AccountSignatureScheme::ALL {
                if other != scheme {
                    assert!(Address::derive(scheme, &vec![0; other.public_key_size()]).is_err());
                }
            }
        }
    }

    #[test]
    fn mldsa_address_payloads_preserve_the_existing_framed_hash_vectors() {
        // Fixed literal byte fixtures, independent of key generation. These
        // freeze structural derivation, not proof of possession of these keys.
        // Expected SHA3-256 bytes were computed independently with Python hashlib:
        // tag || u64_le(1 + key_size) || scheme_id || public_key.
        let fixtures: [(AccountSignatureScheme, &[u8], &str); 3] = [
            (
                AccountSignatureScheme::MlDsa44,
                &[0xa5; 1312],
                "d046f49144482f6f14734f95c46041ab74bb1453c1485732607a182ba6a8a6c5",
            ),
            (
                AccountSignatureScheme::MlDsa65,
                &[0xa5; 1952],
                "9f2f43540f5f717ab6d300b0bf8a65f81a5ec0373ad48cf7cebf8340ffba01fe",
            ),
            (
                AccountSignatureScheme::MlDsa87,
                &[0xa5; 2592],
                "3008d49947ff92fe8ff8497f15569ce1c632c427e837cce79abeb589bdb0aad5",
            ),
        ];
        for (scheme, bytes, expected) in fixtures {
            let address = Address::derive(scheme, bytes).unwrap();
            assert_eq!(hex::encode(address.as_bytes()), expected);
            let key = PublicKey {
                account: scheme,
                bytes: bytes.to_vec(),
            };
            assert_eq!(address_from_public_key(&key), Ok(address));
        }
    }

    #[test]
    fn address_roundtrip() {
        let address = Address([7; ADDRESS_SIZE]);

        let encoded = address_to_string(&address);

        assert_eq!(encoded.len(), ADDRESS_STRING_LEN);
        assert_eq!(address_from_string(&encoded), Ok(address));
    }

    #[test]
    fn zero_address_roundtrip() {
        let address = Address::ZERO;

        let encoded = address_to_string(&address);

        assert_eq!(encoded.len(), ADDRESS_STRING_LEN);
        assert_eq!(address_from_string(&encoded), Ok(address));
    }

    #[test]
    fn base56_preserves_leading_zeroes() {
        let mut payload = [0_u8; ADDRESS_SIZE];

        payload[ADDRESS_SIZE - 1] = 1;

        let encoded = xparq_encode(&payload);

        assert_eq!(encoded.len(), ADDRESS_ENCODED_SIZE);
        assert_eq!(xparq_decode(&encoded).unwrap(), payload);
    }

    #[test]
    fn maximum_payload_roundtrip() {
        let payload = [0xff_u8; ADDRESS_SIZE];

        let encoded = xparq_encode(&payload);

        assert_eq!(encoded.len(), ADDRESS_ENCODED_SIZE);
        assert_eq!(xparq_decode(&encoded).unwrap(), payload);
    }

    #[test]
    fn invalid_character_is_rejected() {
        let invalid = "O".repeat(ADDRESS_ENCODED_SIZE);

        assert_eq!(
            address_from_string(&invalid),
            Err(CryptoError::InvalidAddressEncoding),
        );
    }

    #[test]
    fn ambiguous_characters_are_rejected() {
        for character in [b'O', b'o', b'I', b'i', b'L', b'l'] {
            assert_eq!(xparq_digit(character), None);
        }
    }

    #[test]
    fn alphabet_has_expected_size() {
        assert_eq!(CHARACTER.len(), 56);
    }
}
