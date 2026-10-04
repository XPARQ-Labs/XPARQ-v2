use std::{error::Error, fmt};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CryptoError {
    InvalidAddressEncoding,
    InvalidKeyDerivationParameters,
    InvalidPublicKey,
    InvalidSignatureEncoding,
    InvalidPoWParameters,
    PoWHashFailed,
    VerificationFailed,
}

impl fmt::Display for CryptoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidAddressEncoding => f.write_str("address string is invalid"),
            Self::InvalidKeyDerivationParameters => {
                f.write_str("key derivation parameters are invalid")
            }
            Self::InvalidPublicKey => f.write_str("public key bytes are invalid"),
            Self::InvalidSignatureEncoding => {
                #[cfg(feature = "sqisign-blockchain-test")]
                return f.write_str("signature bytes are not valid SQIsign Level 5 encoding");

                #[cfg(not(feature = "sqisign-blockchain-test"))]
                f.write_str("signature bytes are not valid Signature encoding")
            }
            Self::InvalidPoWParameters => f.write_str("proof-of-work hash parameters are invalid"),
            Self::PoWHashFailed => f.write_str("proof-of-work hash failed"),
            Self::VerificationFailed => f.write_str("signature verification failed"),
        }
    }
}

impl Error for CryptoError {}
