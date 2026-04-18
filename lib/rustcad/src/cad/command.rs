//! CAD-layer undo/redo, layered on [`crate::undo`].
//!
//! The generic undo machinery lives in [`crate::undo`]; this module
//! pins it to a CAD-shaped context and error family so every CAD
//! operation can be written against one familiar pair of types:
//!
//! ```text
//! impl Command<CadContext, CadError> for MyOperation { ... }
//! ```
//!
//! The context bundles the feature tree and (optionally) the B-Rep
//! cache into a single mutable target. Engines that don't care
//! about one half can use [`CadContext`] defaults — commands that
//! only touch the feature tree leave the B-Rep cache alone.

use thiserror::Error;

use super::boolean::BooleanError;
use super::constraint::SolverError;
use super::core::{DependencyGraph, EntityId, GraphError};
use super::kernel::Brep;
use super::parametric::FeatureTree;
use crate::undo;

/// Shared mutable state every CAD command operates on.
///
/// The B-Rep cache is split out from the feature tree so that
/// recompute can mutate the cache without touching user-authored
/// history, which matters for clean undo scopes.
#[derive(Default)]
pub struct CadContext {
    /// Authored feature history.
    pub tree:        FeatureTree,
    /// Evaluated B-Rep geometry.
    pub brep:        Brep,
    /// Graph binding feature ids to the geometry they produced. Used
    /// to invalidate downstream features on edit.
    pub dependencies: DependencyGraph,
}

/// Error family surfaced by CAD commands.
#[derive(Debug, Error)]
pub enum CadError {
    /// The graph tried to reference or mutate a missing entity.
    #[error("entity {0} was not found")]
    MissingEntity(EntityId),
    /// Constraint-solver failure (no-convergence, singular system,
    /// …). See [`SolverError`] for specifics.
    #[error(transparent)]
    Solver(#[from] SolverError),
    /// Boolean-kernel failure. See [`BooleanError`] for specifics.
    #[error(transparent)]
    Boolean(#[from] BooleanError),
    /// Dependency-graph failure.
    #[error(transparent)]
    Graph(#[from] GraphError),
    /// Anything else — a validation failure, a precondition, an
    /// unimplemented path. Free-form message.
    #[error("invalid state: {0}")]
    InvalidState(&'static str),
}

/// Alias for the generic undo command trait pinned to the CAD
/// context + error pair. Every CAD operation implements this.
pub type Command = dyn undo::Command<CadContext, CadError>;

/// Alias for [`crate::undo::CommandStack`] pinned to the CAD
/// context + error pair.
pub type CommandStack = undo::CommandStack<CadContext, CadError>;

/// Utility command that captures + restores a [`CadContext`]
/// snapshot. Useful as a scaffold for operations that can't (yet)
/// surgically track their changes — apply the op, take a snapshot
/// on first apply, roll the whole context back on undo.
///
/// Not a long-term solution: large B-Reps make whole-context
/// snapshots expensive. Real operations should implement [`Command`]
/// with per-field restore instead.
pub struct SnapshotCommand<F>
where
    F: FnMut(&mut CadContext) -> Result<(), CadError>,
{
    label:    &'static str,
    snapshot: Option<CadContextSnapshot>,
    run:      F,
}

impl<F> SnapshotCommand<F>
where
    F: FnMut(&mut CadContext) -> Result<(), CadError>,
{
    /// Construct a snapshot-backed command.
    pub fn new(label: &'static str, run: F) -> Self {
        Self {
            label,
            snapshot: None,
            run,
        }
    }
}

impl<F> undo::Command<CadContext, CadError> for SnapshotCommand<F>
where
    F: FnMut(&mut CadContext) -> Result<(), CadError>,
{
    fn label(&self) -> &'static str {
        self.label
    }

    fn apply(&mut self, ctx: &mut CadContext) -> Result<(), CadError> {
        if self.snapshot.is_none() {
            self.snapshot = Some(CadContextSnapshot::capture(ctx));
        }
        (self.run)(ctx)
    }

    fn undo(&mut self, ctx: &mut CadContext) -> Result<(), CadError> {
        let snapshot = self
            .snapshot
            .as_ref()
            .ok_or(CadError::InvalidState("undo before apply"))?;
        snapshot.restore(ctx);
        Ok(())
    }
}

/// Whole-context snapshot. Heavy but simple — see
/// [`SnapshotCommand`] for when to use it.
struct CadContextSnapshot {
    tree:  Vec<(EntityId, super::parametric::Feature)>,
    graph: DependencyGraph,
    brep:  Brep,
}

impl CadContextSnapshot {
    fn capture(ctx: &CadContext) -> Self {
        Self {
            tree:  ctx.tree.features.clone(),
            graph: ctx.dependencies.clone(),
            brep:  ctx.brep.clone(),
        }
    }

    fn restore(&self, ctx: &mut CadContext) {
        ctx.tree.features = self.tree.clone();
        ctx.dependencies = self.graph.clone();
        ctx.brep = self.brep.clone();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cad::parametric::Feature;
    use crate::cad::sketch::Profile;

    #[test]
    fn snapshot_command_round_trips_feature_insert() {
        let mut ctx = CadContext::default();
        let mut stack: CommandStack = CommandStack::default();
        let cmd = SnapshotCommand::new("insert feature", |ctx| {
            ctx.tree
                .push(
                    EntityId(1),
                    Feature::InlineProfile(Profile { points: vec![] }),
                )
                .map_err(CadError::from)
        });
        stack.execute(&mut ctx, Box::new(cmd)).unwrap();
        assert_eq!(ctx.tree.features.len(), 1);

        stack.undo(&mut ctx).unwrap();
        assert_eq!(ctx.tree.features.len(), 0);

        stack.redo(&mut ctx).unwrap();
        assert_eq!(ctx.tree.features.len(), 1);
    }

    #[test]
    fn snapshot_command_round_trips_brep_mutation() {
        use glam::Vec3;
        let mut ctx = CadContext::default();
        let mut stack: CommandStack = CommandStack::default();
        // Seed the B-Rep so the baseline isn't empty.
        ctx.brep.add_vertex(Vec3::ZERO);
        assert_eq!(ctx.brep.vertices.len(), 1);

        let cmd = SnapshotCommand::new("add vertices", |ctx| {
            ctx.brep.add_vertex(Vec3::X);
            ctx.brep.add_vertex(Vec3::Y);
            Ok(())
        });
        stack.execute(&mut ctx, Box::new(cmd)).unwrap();
        assert_eq!(ctx.brep.vertices.len(), 3);

        stack.undo(&mut ctx).unwrap();
        assert_eq!(ctx.brep.vertices.len(), 1, "undo must restore the full B-Rep");

        stack.redo(&mut ctx).unwrap();
        assert_eq!(ctx.brep.vertices.len(), 3);
    }
}
