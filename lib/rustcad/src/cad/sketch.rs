//! 2D parametric sketches — primitives, closed-profile extraction,
//! and a thin bridge into [`crate::cad::constraint`].
//!
//! A sketch is a flat dictionary of 2D primitives (points, lines,
//! arcs, circles) keyed by [`EntityId`]. Constraints are held as
//! opaque boxed [`Constraint`]s, so the constraint solver can drive
//! the element coordinates without the sketch layer having to know
//! what kinds of constraints exist.
//!
//! Closed-profile extraction — turning a soup of line segments into
//! ordered closed loops ready for [`crate::cad::parametric`] to
//! extrude — is the main value-add here. [`extract_profiles`] builds
//! a simple adjacency map and walks chains greedily.

use std::collections::HashMap;

use glam::Vec2;
use thiserror::Error;

use super::core::EntityId;
use crate::cad::constraint::Constraint;

/// A single 2D sketch primitive. Points carry position only;
/// parametric primitives (Line, Arc, Circle) are defined purely by
/// their anchor points so every dof ends up in the shared variable
/// vector the solver operates on.
#[derive(Debug, Clone)]
pub enum SketchElement {
    /// Free 2D point.
    Point(Vec2),
    /// Line segment between two points.
    Line {
        /// Start point, relative to sketch origin.
        a: Vec2,
        /// End point, relative to sketch origin.
        b: Vec2,
    },
    /// Arc specified by center, radius, and an angular sweep.
    Arc {
        /// Arc center.
        center:      Vec2,
        /// Arc radius.
        radius:      f32,
        /// Start angle in radians, measured from +X.
        start_angle: f32,
        /// End angle in radians. Arc sweeps counter-clockwise from
        /// `start_angle` to `end_angle`.
        end_angle:   f32,
    },
    /// Full circle — shorthand for an arc that sweeps 2π.
    Circle {
        /// Circle center.
        center: Vec2,
        /// Circle radius.
        radius: f32,
    },
}

/// A 2D sketch: primitives + constraints that bind their coordinates.
#[derive(Default)]
pub struct Sketch {
    /// Every primitive keyed by its entity id.
    pub elements:    HashMap<EntityId, SketchElement>,
    /// Constraints acting on the flattened variable vector — see
    /// [`flatten_variables`](Self::flatten_variables).
    pub constraints: Vec<Box<dyn Constraint>>,
}

impl Sketch {
    /// Empty sketch.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add an element and return its id.
    pub fn add(&mut self, id: EntityId, element: SketchElement) {
        self.elements.insert(id, element);
    }

    /// Remove an element by id. Returns the removed element if it
    /// existed.
    pub fn remove(&mut self, id: EntityId) -> Option<SketchElement> {
        self.elements.remove(&id)
    }

    /// Queue a constraint to be solved on the next constraint pass.
    pub fn add_constraint(&mut self, constraint: Box<dyn Constraint>) {
        self.constraints.push(constraint);
    }

    /// Pack every primitive's variable-valued coordinates into a
    /// single flat `f64` vector suitable for
    /// [`crate::cad::constraint::solve_gauss_newton`]. The
    /// per-element variable indices are returned so callers can map
    /// solver output back into the sketch.
    pub fn flatten_variables(&self) -> (Vec<f64>, HashMap<EntityId, Vec<usize>>) {
        let mut vars = Vec::new();
        let mut index_map = HashMap::new();
        for (id, element) in &self.elements {
            let mut indices = Vec::new();
            match element {
                SketchElement::Point(p) => {
                    indices.push(vars.len());
                    vars.push(p.x as f64);
                    indices.push(vars.len());
                    vars.push(p.y as f64);
                }
                SketchElement::Line { a, b } => {
                    for v in [a.x, a.y, b.x, b.y] {
                        indices.push(vars.len());
                        vars.push(v as f64);
                    }
                }
                SketchElement::Arc {
                    center,
                    radius,
                    start_angle,
                    end_angle,
                } => {
                    for v in [center.x, center.y, *radius, *start_angle, *end_angle] {
                        indices.push(vars.len());
                        vars.push(v as f64);
                    }
                }
                SketchElement::Circle { center, radius } => {
                    for v in [center.x, center.y, *radius] {
                        indices.push(vars.len());
                        vars.push(v as f64);
                    }
                }
            }
            index_map.insert(*id, indices);
        }
        (vars, index_map)
    }
}

/// An ordered closed loop of 2D points, suitable for input to
/// [`crate::cad::parametric::Feature::Extrude`].
#[derive(Debug, Clone, PartialEq)]
pub struct Profile {
    /// Closed polyline vertices, traversed in order. Implicitly
    /// closes from the last element back to the first.
    pub points: Vec<Vec2>,
}

/// Failure modes of [`extract_profiles`].
#[derive(Debug, Clone, PartialEq, Error)]
pub enum ProfileError {
    /// The sketch contains disconnected chains that couldn't be
    /// resolved into closed loops.
    #[error("sketch contains open chains; expected closed profiles")]
    OpenChain,
}

/// Extract closed polyline profiles from the sketch's line segments.
///
/// Arcs and circles are treated as primitive profiles directly
/// (circles produce a single-element `Profile` marker — callers
/// decide how to tessellate them). The walker is deliberately
/// simple: it greedily chains line segments whose endpoints
/// coincide within [`crate::cad::math::num::EPS`], and returns
/// [`ProfileError::OpenChain`] if any dangling endpoint remains.
pub fn extract_profiles(sketch: &Sketch) -> Result<Vec<Profile>, ProfileError> {
    use crate::cad::math::num::approx_eq_tol;

    let mut profiles = Vec::new();
    // Circles round-trip as their own closed profiles — we don't
    // discretize them here; downstream tessellation decides how many
    // segments to emit for a given chord tolerance.
    for element in sketch.elements.values() {
        if let SketchElement::Circle { center, radius } = element {
            profiles.push(Profile {
                points: vec![*center + Vec2::new(*radius, 0.0)],
            });
        }
    }

    // Gather every line segment as (a, b) pairs.
    let mut segs: Vec<(Vec2, Vec2)> = sketch
        .elements
        .values()
        .filter_map(|e| {
            if let SketchElement::Line { a, b } = e {
                Some((*a, *b))
            } else {
                None
            }
        })
        .collect();

    let eps = 1e-4;
    while let Some((start, mut cur)) = segs.pop() {
        let mut chain = vec![start, cur];
        loop {
            if approx_eq_tol(cur.x, start.x, eps) && approx_eq_tol(cur.y, start.y, eps) {
                // Closed loop — pop the duplicate endpoint.
                chain.pop();
                profiles.push(Profile { points: chain });
                break;
            }
            let Some(pos) = segs.iter().position(|(a, b)| {
                (approx_eq_tol(a.x, cur.x, eps) && approx_eq_tol(a.y, cur.y, eps))
                    || (approx_eq_tol(b.x, cur.x, eps) && approx_eq_tol(b.y, cur.y, eps))
            }) else {
                return Err(ProfileError::OpenChain);
            };
            let (a, b) = segs.remove(pos);
            let next = if approx_eq_tol(a.x, cur.x, eps) && approx_eq_tol(a.y, cur.y, eps) {
                b
            } else {
                a
            };
            chain.push(next);
            cur = next;
        }
    }

    Ok(profiles)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u64) -> EntityId {
        EntityId(n)
    }

    #[test]
    fn extract_unit_square_profile() {
        let mut sketch = Sketch::new();
        sketch.add(
            id(1),
            SketchElement::Line {
                a: Vec2::new(0.0, 0.0),
                b: Vec2::new(1.0, 0.0),
            },
        );
        sketch.add(
            id(2),
            SketchElement::Line {
                a: Vec2::new(1.0, 0.0),
                b: Vec2::new(1.0, 1.0),
            },
        );
        sketch.add(
            id(3),
            SketchElement::Line {
                a: Vec2::new(1.0, 1.0),
                b: Vec2::new(0.0, 1.0),
            },
        );
        sketch.add(
            id(4),
            SketchElement::Line {
                a: Vec2::new(0.0, 1.0),
                b: Vec2::new(0.0, 0.0),
            },
        );
        let profiles = extract_profiles(&sketch).unwrap();
        assert_eq!(profiles.len(), 1);
        assert_eq!(profiles[0].points.len(), 4);
    }

    #[test]
    fn open_chain_rejected() {
        let mut sketch = Sketch::new();
        sketch.add(
            id(1),
            SketchElement::Line {
                a: Vec2::new(0.0, 0.0),
                b: Vec2::new(1.0, 0.0),
            },
        );
        sketch.add(
            id(2),
            SketchElement::Line {
                a: Vec2::new(1.0, 0.0),
                b: Vec2::new(1.0, 1.0),
            },
        );
        assert_eq!(
            extract_profiles(&sketch).unwrap_err(),
            ProfileError::OpenChain
        );
    }

    #[test]
    fn flatten_packs_every_coordinate() {
        let mut sketch = Sketch::new();
        sketch.add(id(1), SketchElement::Point(Vec2::new(1.0, 2.0)));
        sketch.add(
            id(2),
            SketchElement::Line {
                a: Vec2::ZERO,
                b: Vec2::X,
            },
        );
        let (vars, _map) = sketch.flatten_variables();
        // point: 2 vars, line: 4 vars → 6 total.
        assert_eq!(vars.len(), 6);
    }
}
