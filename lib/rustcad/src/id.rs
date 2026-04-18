//! Typed monotonically-increasing id allocation.
//!
//! One tiny generic struct — [`IdAllocator<T>`] — that hands out
//! freshly-minted ids of whatever newtype the caller defines. Lifted
//! out of `engine::scene::id` so other tooling (asset catalogs, undo
//! receipts, RPC correlation ids) can reuse the same "next integer,
//! wrapped in a typed newtype" pattern without copying the six lines
//! every time.
//!
//! The design rule: the allocator owns the monotonic counter, the
//! caller owns the newtype. Conversion happens through [`From<u64>`]
//! so nothing in this crate needs to know what a `SceneId` or an
//! `AssetId` actually is.
//!
//! # Example
//!
//! ```
//! use rustcad::id::IdAllocator;
//!
//! #[derive(Debug, PartialEq, Eq)]
//! struct AssetId(u64);
//!
//! impl From<u64> for AssetId {
//!     fn from(n: u64) -> Self { Self(n) }
//! }
//!
//! let mut alloc: IdAllocator<AssetId> = IdAllocator::default();
//! assert_eq!(alloc.next(), AssetId(1));
//! assert_eq!(alloc.next(), AssetId(2));
//! ```

use std::marker::PhantomData;

/// Hands out fresh, strictly-increasing ids of type `T`.
///
/// The first call to [`next`](IdAllocator::next) returns `T::from(1)`
/// (or `T::from(start_at + 1)` if constructed with
/// [`new`](IdAllocator::new)). Ids are never reused, so a dropped-then-
/// re-added entity gets a new id rather than recycling the old one —
/// that keeps undo / serialization from conflating two different
/// "entity 42"s across history.
///
/// Cheap to `Clone`/`Copy`: it's a single `u64` plus a zero-sized
/// marker. Not thread-safe; wrap in `Mutex` or hand each thread its own
/// if you need parallel allocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IdAllocator<T> {
    next:    u64,
    _marker: PhantomData<fn() -> T>,
}

impl<T> Default for IdAllocator<T> {
    fn default() -> Self {
        Self {
            next:    0,
            _marker: PhantomData,
        }
    }
}

impl<T> IdAllocator<T> {
    /// Build an allocator whose first issued id is `start_at + 1`.
    ///
    /// Useful for resuming after deserialization: pass the highest id
    /// you saw on disk so the next `next()` won't collide with an
    /// existing one.
    pub fn new(start_at: u64) -> Self {
        Self {
            next:    start_at,
            _marker: PhantomData,
        }
    }

    /// Highest id ever handed out. `0` on a fresh allocator — the
    /// first call to [`next`](Self::next) will return `T::from(1)`.
    pub fn peek(&self) -> u64 {
        self.next
    }
}

impl<T: From<u64>> IdAllocator<T> {
    /// Mint a new id. Increments the internal counter and wraps the
    /// new value in `T` via [`From<u64>`].
    pub fn next(&mut self) -> T {
        self.next += 1;
        T::from(self.next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq, Eq)]
    struct Id(u64);

    impl From<u64> for Id {
        fn from(n: u64) -> Self {
            Self(n)
        }
    }

    #[test]
    fn default_starts_at_one() {
        let mut a: IdAllocator<Id> = IdAllocator::default();
        assert_eq!(a.next(), Id(1));
        assert_eq!(a.next(), Id(2));
    }

    #[test]
    fn new_resumes_from_offset() {
        let mut a: IdAllocator<Id> = IdAllocator::new(41);
        assert_eq!(a.next(), Id(42));
    }

    #[test]
    fn peek_matches_last_issued() {
        let mut a: IdAllocator<Id> = IdAllocator::default();
        a.next();
        a.next();
        a.next();
        assert_eq!(a.peek(), 3);
    }
}
