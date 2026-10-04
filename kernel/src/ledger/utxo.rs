use std::{collections::BTreeMap, error::Error as StdError, fmt};

use borsh::{BorshDeserialize, BorshSerialize};
use crypto::Address;

use crate::monetary::coin::{CoinShare, Zeno};

#[derive(Debug, Clone, Copy, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct CoinUtxo {
    pub amount: Zeno,
    pub owner: Address,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct UtxoSet {
    coins: BTreeMap<CoinShare, CoinUtxo>,
    total_value: Zeno,
}

impl UtxoSet {
    pub fn coin(&self, id: &CoinShare) -> Option<&CoinUtxo> {
        self.coins.get(id)
    }

    pub fn total_value(&self) -> Zeno {
        self.total_value
    }

    pub(crate) fn insert_coin(&mut self, id: CoinShare, coin: CoinUtxo) -> Result<(), Error> {
        if coin.amount.is_zero() {
            return Err(Error::ZeroAmount);
        }
        if self.coins.contains_key(&id) {
            return Err(Error::CoinCollision);
        }

        let total_value = self
            .total_value
            .checked_add(coin.amount)
            .ok_or(Error::ValueOverflow)?;

        self.coins.insert(id, coin);
        self.total_value = total_value;

        Ok(())
    }

    pub(crate) fn consume_coin(&mut self, id: &CoinShare) -> Result<CoinUtxo, Error> {
        let coin = self.coins.get(id).copied().ok_or(Error::NotFound)?;

        let total_value = self
            .total_value
            .checked_sub(coin.amount)
            .ok_or(Error::ValueUnderflow)?;

        self.coins.remove(id);
        self.total_value = total_value;

        Ok(coin)
    }

    pub fn coins(&self) -> impl Iterator<Item = (CoinShare, &CoinUtxo)> + '_ {
        self.coins.iter().map(|(&id, coin)| (id, coin))
    }

    pub fn len(&self) -> usize {
        self.coins.len()
    }

    pub fn is_empty(&self) -> bool {
        self.coins.is_empty()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    ZeroAmount,
    NotFound,
    CoinCollision,
    ValueOverflow,
    ValueUnderflow,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroAmount => f.write_str("coin UTXO amount must be nonzero"),
            Self::NotFound => f.write_str("UTXO was not found"),
            Self::CoinCollision => f.write_str("coin UTXO ID already exists"),
            Self::ValueOverflow => f.write_str("coin UTXO total value overflowed"),
            Self::ValueUnderflow => f.write_str("coin UTXO total value underflowed"),
        }
    }
}

impl StdError for Error {}

#[cfg(test)]
mod invariant_tests {
    use super::*;

    fn coin(amount: u64) -> CoinUtxo {
        CoinUtxo {
            owner: Address([1; crypto::ADDRESS_SIZE]),
            amount: Zeno::from_zeno(amount),
        }
    }

    fn id(value: u64) -> CoinShare {
        let mut bytes = [0; crypto::HASH16_SIZE];
        bytes[..8].copy_from_slice(&value.to_le_bytes());
        CoinShare::from_bytes(bytes)
    }

    #[test]
    fn rejected_insertions_leave_the_map_and_cache_unchanged() {
        let mut set = UtxoSet::default();
        let empty = set.clone();
        assert_eq!(set.insert_coin(id(1), coin(0)), Err(Error::ZeroAmount));
        assert_eq!(set, empty);
        set.insert_coin(id(1), coin(u64::MAX)).unwrap();
        let before = set.clone();
        assert_eq!(set.insert_coin(id(1), coin(1)), Err(Error::CoinCollision));
        assert_eq!(set, before);
        assert_eq!(set.insert_coin(id(2), coin(1)), Err(Error::ValueOverflow));
        assert_eq!(set, before);
        assert_eq!(set.consume_coin(&id(2)), Err(Error::NotFound));
        assert_eq!(set, before);
    }

    #[test]
    fn generated_insert_consume_sequences_match_an_independent_model() {
        for seed in 1..=16u64 {
            let mut random = seed;
            let mut set = UtxoSet::default();
            let mut model = BTreeMap::new();
            for step in 0..256 {
                random ^= random << 13;
                random ^= random >> 7;
                random ^= random << 17;
                if random & 1 == 0 || model.is_empty() {
                    let key = id(step);
                    let value = coin(random % 10_000 + 1);
                    set.insert_coin(key, value).unwrap();
                    model.insert(key, value);
                } else {
                    let offset = random as usize % model.len();
                    let key = *model.keys().nth(offset).unwrap();
                    assert_eq!(set.consume_coin(&key).unwrap(), model.remove(&key).unwrap());
                }
                let actual: BTreeMap<_, _> =
                    set.coins().map(|(key, value)| (key, *value)).collect();
                assert_eq!(actual, model);
                let total: u64 = model.values().map(|value| value.amount.as_zeno()).sum();
                assert_eq!(set.total_value().as_zeno(), total);
                let encoded = borsh::to_vec(&set).unwrap();
                assert_eq!(UtxoSet::try_from_slice(&encoded).unwrap(), set);
            }
        }
    }
}
