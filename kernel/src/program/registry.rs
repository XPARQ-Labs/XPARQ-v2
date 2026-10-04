use std::collections::BTreeMap;

use borsh::{BorshDeserialize, BorshSerialize};
use crypto::{Address, HASH_SIZE, HashDomain, canonical_bytes, domain};

use crate::common::Height;

/// Hash identifying deployed code in the registry, distinct from SystemProgramId routes.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, BorshSerialize, BorshDeserialize,
)]
pub struct ProgramId([u8; HASH_SIZE]);

impl ProgramId {
    pub const fn from_bytes(bytes: [u8; HASH_SIZE]) -> Self {
        Self(bytes)
    }

    pub const fn as_bytes(&self) -> &[u8; HASH_SIZE] {
        &self.0
    }

    pub const fn into_bytes(self) -> [u8; HASH_SIZE] {
        self.0
    }

    pub fn derive(
        owner: Address,
        nonce: u64,
        code_hash: ProgramHash,
    ) -> Result<Self, RegistryError> {
        let bytes = canonical_bytes(&(b"xparq:program-id:v1", owner, nonce, code_hash))
            .map_err(|_| RegistryError::Encoding)?;

        Ok(Self(domain(HashDomain::XPARQArtifact, &bytes).into_bytes()))
    }
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, BorshSerialize, BorshDeserialize,
)]
pub struct ProgramHash([u8; HASH_SIZE]);

impl ProgramHash {
    pub const fn from_bytes(bytes: [u8; HASH_SIZE]) -> Self {
        Self(bytes)
    }

    pub const fn as_bytes(&self) -> &[u8; HASH_SIZE] {
        &self.0
    }

    pub const fn into_bytes(self) -> [u8; HASH_SIZE] {
        self.0
    }

    pub fn derive(code: &[u8]) -> Result<Self, RegistryError> {
        let bytes = canonical_bytes(&(b"xparq:program-code:v1", code.to_vec()))
            .map_err(|_| RegistryError::Encoding)?;

        Ok(Self(domain(HashDomain::XPARQArtifact, &bytes).into_bytes()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct ProgramRecord {
    pub code_hash: ProgramHash,
    pub code: Vec<u8>,
    pub owner: Address,
    pub nonce: u64,
    pub deployed_at: Height,
    pub state_value: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct ProgramRegistry {
    programs: BTreeMap<ProgramId, ProgramRecord>,
}

impl ProgramRegistry {
    pub fn program(&self, id: &ProgramId) -> Option<&ProgramRecord> {
        self.programs.get(id)
    }

    pub fn contains(&self, id: &ProgramId) -> bool {
        self.programs.contains_key(id)
    }

    pub(crate) fn insert(
        &mut self,
        id: ProgramId,
        record: ProgramRecord,
    ) -> Result<(), RegistryError> {
        if self.programs.contains_key(&id)
            || self
                .programs
                .values()
                .any(|existing| existing.owner == record.owner && existing.nonce == record.nonce)
        {
            return Err(RegistryError::AlreadyExists);
        }

        self.programs.insert(id, record);
        Ok(())
    }

    pub(crate) fn remove(&mut self, id: &ProgramId) -> Option<ProgramRecord> {
        self.programs.remove(id)
    }

    pub fn len(&self) -> usize {
        self.programs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.programs.is_empty()
    }

    pub(crate) fn set_state(&mut self, id: ProgramId, value: i64) -> Option<i64> {
        let record = self.programs.get_mut(&id)?;
        Some(std::mem::replace(&mut record.state_value, value))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistryError {
    AlreadyExists,
    Encoding,
}
