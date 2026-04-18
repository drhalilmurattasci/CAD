//! Blender-style modifier stack over [`Mesh`].
//!
//! A modifier is any transformation `Mesh → Mesh` — translate,
//! mirror, array, subdivide. Stacking them expresses non-destructive
//! editing: the base mesh stays untouched and the modifier stack
//! rebuilds the display mesh every frame (or every evaluation).
//!
//! The stack itself ([`ModifierStack`]) is just a `Vec<Box<dyn
//! Modifier>>` with an evaluator; there's no clever graph here on
//! purpose — mesh modifiers are a pure fold.

use glam::Vec3;

use super::math::Plane;
use super::mesh::{Mesh, subdivide_midpoint};

/// A single mesh-to-mesh transformation.
pub trait Modifier {
    /// Human-readable operation name. Shown in UI stacks.
    fn label(&self) -> &'static str;

    /// Run the modifier, producing a new mesh. Implementations are
    /// free to return an owned `Mesh` rather than mutate in place —
    /// stack evaluation shuttles the result into the next modifier.
    fn apply(&self, mesh: &Mesh) -> Mesh;
}

/// Translate every vertex by a fixed offset.
#[derive(Debug, Clone, Copy)]
pub struct TranslateModifier {
    /// World-space offset added to each vertex.
    pub offset: Vec3,
}

impl Modifier for TranslateModifier {
    fn label(&self) -> &'static str {
        "translate"
    }

    fn apply(&self, mesh: &Mesh) -> Mesh {
        let mut out = mesh.clone();
        out.translate(self.offset);
        out
    }
}

/// Scale every vertex component-wise.
#[derive(Debug, Clone, Copy)]
pub struct ScaleModifier {
    /// Per-axis scale factor.
    pub factor: Vec3,
}

impl Modifier for ScaleModifier {
    fn label(&self) -> &'static str {
        "scale"
    }

    fn apply(&self, mesh: &Mesh) -> Mesh {
        let mut out = mesh.clone();
        out.scale(self.factor);
        out
    }
}

/// Mirror the mesh across a plane and merge the result onto the
/// original.
///
/// Reflecting flips triangle winding, so the mirrored copy's
/// indices are reversed before merging — this keeps the combined
/// mesh outward-facing.
#[derive(Debug, Clone, Copy)]
pub struct MirrorModifier {
    /// Mirror plane.
    pub plane: Plane,
}

impl Modifier for MirrorModifier {
    fn label(&self) -> &'static str {
        "mirror"
    }

    fn apply(&self, mesh: &Mesh) -> Mesh {
        let mut mirrored = mesh.clone();
        for p in &mut mirrored.positions {
            *p = self.plane.reflect(*p);
        }
        for tri in &mut mirrored.triangles {
            tri.swap(1, 2); // flip winding
        }
        let mut out = mesh.clone();
        out.merge(&mirrored);
        out
    }
}

/// Replicate the mesh `count` times along a fixed offset.
///
/// Instance 0 is the original; instance `i ≥ 1` is translated by
/// `offset * i`. Useful for fence posts, window mullions, railings,
/// etc.
#[derive(Debug, Clone, Copy)]
pub struct ArrayModifier {
    /// Total number of copies (including the original).
    pub count:  u32,
    /// Offset between consecutive copies.
    pub offset: Vec3,
}

impl Modifier for ArrayModifier {
    fn label(&self) -> &'static str {
        "array"
    }

    fn apply(&self, mesh: &Mesh) -> Mesh {
        if self.count == 0 {
            return Mesh::new();
        }
        let mut out = mesh.clone();
        for i in 1..self.count {
            let mut copy = mesh.clone();
            copy.translate(self.offset * i as f32);
            out.merge(&copy);
        }
        out
    }
}

/// N passes of midpoint triangle subdivision — see
/// [`subdivide_midpoint`].
#[derive(Debug, Clone, Copy)]
pub struct SubdivideModifier {
    /// Number of subdivision passes.
    pub passes: u32,
}

impl Modifier for SubdivideModifier {
    fn label(&self) -> &'static str {
        "subdivide"
    }

    fn apply(&self, mesh: &Mesh) -> Mesh {
        let mut cur = mesh.clone();
        for _ in 0..self.passes {
            cur = subdivide_midpoint(&cur);
        }
        cur
    }
}

/// Ordered chain of modifiers. Apply by folding over
/// [`evaluate`](Self::evaluate).
#[derive(Default)]
pub struct ModifierStack {
    /// Modifiers, applied in order from index 0 upward.
    pub modifiers: Vec<Box<dyn Modifier>>,
}

impl ModifierStack {
    /// Empty stack.
    pub fn new() -> Self {
        Self::default()
    }

    /// Append a modifier to the end of the stack.
    pub fn push(&mut self, modifier: Box<dyn Modifier>) {
        self.modifiers.push(modifier);
    }

    /// Fold every modifier over `base`, producing the final display
    /// mesh. Returns a clone of `base` when the stack is empty.
    pub fn evaluate(&self, base: &Mesh) -> Mesh {
        let mut cur = base.clone();
        for modifier in &self.modifiers {
            cur = modifier.apply(&cur);
        }
        cur
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cad::mesh::Mesh;

    fn base() -> Mesh {
        let mut m = Mesh::new();
        m.push_vertex(Vec3::ZERO);
        m.push_vertex(Vec3::X);
        m.push_vertex(Vec3::Y);
        m.push_triangle([0, 1, 2]);
        m
    }

    #[test]
    fn translate_modifier_shifts_positions() {
        let m = base();
        let shifted = TranslateModifier {
            offset: Vec3::new(2.0, 0.0, 0.0),
        }
        .apply(&m);
        assert!((shifted.positions[0] - Vec3::new(2.0, 0.0, 0.0)).length() < 1e-5);
    }

    #[test]
    fn array_modifier_replicates() {
        let m = base();
        let arr = ArrayModifier {
            count:  3,
            offset: Vec3::new(1.0, 0.0, 0.0),
        }
        .apply(&m);
        assert_eq!(arr.vertex_count(), 9);
        assert_eq!(arr.triangle_count(), 3);
    }

    #[test]
    fn mirror_modifier_doubles_and_reflects() {
        let m = base();
        let mirrored = MirrorModifier {
            plane: Plane::new(Vec3::ZERO, Vec3::X),
        }
        .apply(&m);
        // Original 3 + mirrored 3.
        assert_eq!(mirrored.vertex_count(), 6);
        assert_eq!(mirrored.triangle_count(), 2);
    }

    #[test]
    fn stack_composes_modifiers() {
        let m = base();
        let mut stack = ModifierStack::new();
        stack.push(Box::new(TranslateModifier { offset: Vec3::X }));
        stack.push(Box::new(ScaleModifier {
            factor: Vec3::splat(2.0),
        }));
        let out = stack.evaluate(&m);
        // First translate by X, then scale by 2 → first vertex at (2, 0, 0).
        assert!((out.positions[0] - Vec3::new(2.0, 0.0, 0.0)).length() < 1e-5);
    }
}
