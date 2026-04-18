//! Newtype ids over a shared `u64` counter.
//!
//! Shows the intended consumption pattern: define your own typed id
//! struct with a `From<u64>` impl, then let `IdAllocator<YourId>`
//! hand out fresh values. The allocator never reuses ids, so
//! long-running sessions keep id comparisons meaningful across
//! undo/redo.
//!
//! Run with:
//!
//! ```bash
//! cargo run --example id_alloc
//! ```

use rustcad::id::IdAllocator;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AssetId(u64);

impl From<u64> for AssetId {
    fn from(n: u64) -> Self {
        Self(n)
    }
}

fn main() {
    // Fresh run — counter starts at 0, first issued id is 1.
    let mut alloc: IdAllocator<AssetId> = IdAllocator::default();
    let a = alloc.next();
    let b = alloc.next();
    let c = alloc.next();
    println!("fresh: {a:?} {b:?} {c:?}");

    // Simulate resuming after a save: the last id on disk was 100, so
    // bootstrap the allocator with `new(100)` to avoid colliding.
    let mut resumed: IdAllocator<AssetId> = IdAllocator::new(100);
    println!("resumed: {:?}", resumed.next()); // AssetId(101)
    println!("high-water: {}", resumed.peek());
}
