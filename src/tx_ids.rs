//! A set of tx ids whose memory has a ceiling.
//!
//! A hash set costs 5 to 10 bytes per id with no upper bound. A bitset over
//! the whole u32 range is a fixed 512 MiB, but scattered ids fill it after
//! only a few million. So the set starts as a hash set and switches to the
//! bitset once the hash set would be about as big.

use std::collections::HashSet;
use std::fmt;

use crate::TxId;

/// 2^24 ids (about 16.8 million) take about 160 MiB as a hash set, and its
/// next growth step would reach 320 MiB.
const SPARSE_LIMIT: usize = 1 << 24;

#[derive(Clone)]
pub struct TxIdSet(Repr);

#[derive(Clone)]
enum Repr {
    Sparse(HashSet<TxId>),
    Dense(Bitset),
}

impl Default for TxIdSet {
    fn default() -> Self {
        Self(Repr::Sparse(HashSet::new()))
    }
}

impl TxIdSet {
    pub fn contains(&self, tx: TxId) -> bool {
        match &self.0 {
            Repr::Sparse(ids) => ids.contains(&tx),
            Repr::Dense(bits) => bits.contains(tx),
        }
    }

    /// Adds `tx` and returns whether it was new.
    pub fn insert(&mut self, tx: TxId) -> bool {
        if let Repr::Sparse(ids) = &self.0 {
            if ids.len() >= SPARSE_LIMIT {
                self.make_dense();
            }
        }
        match &mut self.0 {
            Repr::Sparse(ids) => ids.insert(tx),
            Repr::Dense(bits) => bits.insert(tx),
        }
    }

    fn make_dense(&mut self) {
        if let Repr::Sparse(ids) = &self.0 {
            let mut bits = Bitset::default();
            for &id in ids {
                bits.insert(id);
            }
            self.0 = Repr::Dense(bits);
        }
    }
}

// The contents can be millions of ids, so only the size is shown.
impl fmt::Debug for TxIdSet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0 {
            Repr::Sparse(ids) => write!(f, "TxIdSet(sparse, {} ids)", ids.len()),
            Repr::Dense(_) => write!(f, "TxIdSet(dense)"),
        }
    }
}

/// The high 16 bits of an id pick its chunk, the low 16 its bit.
const CHUNK_BITS: u32 = 16;
const CHUNKS: usize = 1 << (32 - CHUNK_BITS);
const WORDS_PER_CHUNK: usize = (1 << CHUNK_BITS) / 64;

/// One bit per possible id, in 8 KiB chunks allocated when first touched,
/// so clustered ids stay cheap.
#[derive(Clone)]
struct Bitset {
    chunks: Vec<Option<Box<[u64; WORDS_PER_CHUNK]>>>,
}

impl Default for Bitset {
    fn default() -> Self {
        Self {
            chunks: vec![None; CHUNKS],
        }
    }
}

impl Bitset {
    fn contains(&self, tx: TxId) -> bool {
        let (chunk, word, bit) = locate(tx);
        self.chunks[chunk]
            .as_ref()
            .is_some_and(|words| words[word] & bit != 0)
    }

    fn insert(&mut self, tx: TxId) -> bool {
        let (chunk, word, bit) = locate(tx);
        let words = self.chunks[chunk].get_or_insert_with(|| Box::new([0; WORDS_PER_CHUNK]));
        let new = words[word] & bit == 0;
        words[word] |= bit;
        new
    }
}

/// Returns the chunk, the word within it and the bit within that word.
fn locate(TxId(tx): TxId) -> (usize, usize, u64) {
    let low = tx & ((1 << CHUNK_BITS) - 1);
    (
        (tx >> CHUNK_BITS) as usize,
        (low / 64) as usize,
        1 << (low % 64),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Both edges of the id space, and both sides of a word and a chunk
    /// boundary.
    const EDGES: [u32; 7] = [0, 63, 64, 65_535, 65_536, u32::MAX - 1, u32::MAX];
    const NEIGHBOURS: [u32; 6] = [1, 62, 65, 65_534, 65_537, u32::MAX - 2];

    #[test]
    fn starts_empty() {
        let set = TxIdSet::default();
        assert!(!set.contains(TxId(0)));
        assert!(!set.contains(TxId(u32::MAX)));
    }

    #[test]
    fn insert_reports_whether_the_id_was_new() {
        let mut set = TxIdSet::default();
        assert!(set.insert(TxId(7)));
        assert!(!set.insert(TxId(7)));
        assert!(set.contains(TxId(7)));
    }

    #[test]
    fn neighbouring_ids_do_not_overlap() {
        let mut sparse = TxIdSet::default();
        let mut dense = TxIdSet::default();
        dense.make_dense();
        for set in [&mut sparse, &mut dense] {
            for id in EDGES {
                set.insert(TxId(id));
            }
            for id in EDGES {
                assert!(set.contains(TxId(id)), "{id} missing from {set:?}");
            }
            for id in NEIGHBOURS {
                assert!(!set.contains(TxId(id)), "{id} present in {set:?}");
            }
        }
    }

    #[test]
    fn making_the_set_dense_keeps_every_id() {
        let mut set = TxIdSet::default();
        let ids = [0, 7, 65_536, 3_000_000_000, u32::MAX].map(TxId);
        for id in ids {
            set.insert(id);
        }
        set.make_dense();
        assert!(matches!(set.0, Repr::Dense(_)));
        for id in ids {
            assert!(set.contains(id));
            assert!(!set.insert(id), "{id:?} reported as new");
        }
        assert!(set.insert(TxId(8)));
    }
}
