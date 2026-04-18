use std::fmt;

use serde::{Deserialize, Serialize};

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default,
)]
pub struct SceneId(pub u64);

impl SceneId {
    pub fn new(value: u64) -> Self {
        Self(value)
    }
}

impl fmt::Display for SceneId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<u64> for SceneId {
    fn from(value: u64) -> Self {
        Self(value)
    }
}

/// Scene-local id allocator. Thin alias over the generic
/// [`rustcad::id::IdAllocator`] pinned to [`SceneId`] — see that type
/// for the allocation semantics (monotonic, never-reused, resumable
/// via [`IdAllocator::new`]).
pub type IdAllocator = rustcad::id::IdAllocator<SceneId>;
