//! Thin engine-side alias over [`rustcad::undo`].
//!
//! The generic machinery — [`rustcad::undo::Command`] /
//! [`rustcad::undo::CommandStack`] — lives in the framework-agnostic
//! `rustcad` crate so downstream tools can reuse it. Here we pin the
//! type parameters to the scene document + our domain error enum, so
//! engine code keeps writing `impl Command for …` and `CommandStack`
//! without mentioning the generics on every line.
//!
//! Concrete commands live in sibling modules (`entity`, `transform`,
//! `component`, `prefab`) and `impl Command<SceneDocument,
//! CommandError>` directly against the rustcad trait.

use thiserror::Error;

use crate::scene::{SceneDocument, SceneId};

/// Re-export of the generic [`rustcad::undo::Command`] trait. Engine
/// callers typically work with the concrete form
/// `Command<SceneDocument, CommandError>`.
pub use rustcad::undo::Command;

/// Error family surfaced by the engine's command implementations.
/// Deliberately kept a small closed enum so UI layers can
/// pattern-match on it rather than stringly-comparing.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum CommandError {
    #[error("entity `{0}` was not found")]
    MissingEntity(SceneId),
    #[error("command precondition failed: {0}")]
    InvalidState(&'static str),
}

/// Undo / redo history pinned to the scene document + engine error
/// enum. See [`rustcad::undo::CommandStack`] for the underlying
/// semantics.
pub type CommandStack = rustcad::undo::CommandStack<SceneDocument, CommandError>;
