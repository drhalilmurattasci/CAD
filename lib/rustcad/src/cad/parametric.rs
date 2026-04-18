//! Feature tree — the FreeCAD parametric layer.
//!
//! A feature tree is the user-visible history of CAD operations.
//! Edit an early feature, the downstream features recompute. The
//! engine here is deliberately lightweight: an ordered list of
//! [`Feature`]s plus a [`DependencyGraph`] tracking which features
//! feed which. Recompute order comes straight from the graph's
//! topological sort.
//!
//! Actual execution of a feature (extruding a profile, applying a
//! boolean cut, …) is delegated to whatever CAD backend the
//! consumer plugs in — the parametric layer only owns the *schedule*.

use glam::Vec3;

use super::core::{DependencyGraph, EntityId, Node};
use super::sketch::Profile;

/// A parametric modeling operation. Variants carry the parameters
/// the operation needs; references to upstream geometry use
/// [`EntityId`] so the dependency graph stays serializable.
#[derive(Debug, Clone)]
pub enum Feature {
    /// Push a sketch profile along a direction to create a solid.
    Extrude {
        /// Upstream sketch / profile id.
        profile:   EntityId,
        /// Total extrusion distance along the profile's surface
        /// normal.
        distance:  f32,
        /// Whether the extrusion is symmetric about the profile
        /// plane.
        symmetric: bool,
    },
    /// Subtract the swept volume from the current model.
    Cut {
        /// Upstream profile id.
        profile:  EntityId,
        /// Cut depth.
        distance: f32,
    },
    /// Revolve a profile around an axis.
    Revolve {
        /// Upstream profile id.
        profile: EntityId,
        /// Axis origin.
        axis_origin: Vec3,
        /// Axis direction.
        axis_dir: Vec3,
        /// Sweep angle in radians.
        angle:   f32,
    },
    /// Sweep a profile along a path.
    Sweep {
        /// Cross-section profile id.
        profile: EntityId,
        /// Sweep path (typically another sketch's single-wire).
        path:    EntityId,
    },
    /// Loft between two or more cross-section profiles.
    Loft {
        /// Ordered profiles from start to end.
        profiles: Vec<EntityId>,
    },
    /// Round the listed edges to a constant radius.
    Fillet {
        /// Edge ids to fillet.
        edges:  Vec<EntityId>,
        /// Fillet radius.
        radius: f32,
    },
    /// Chamfer the listed edges.
    Chamfer {
        /// Edge ids to chamfer.
        edges:    Vec<EntityId>,
        /// Setback distance.
        distance: f32,
    },
    /// Inline cached profile (for tests / examples that don't want
    /// to go through a full sketch layer). Carries the profile
    /// directly instead of an upstream id.
    InlineProfile(Profile),
}

impl Feature {
    /// Every upstream entity this feature depends on. Returned in no
    /// particular order — the parametric engine sorts via the
    /// dependency graph.
    pub fn dependencies(&self) -> Vec<EntityId> {
        match self {
            Feature::Extrude { profile, .. }
            | Feature::Cut { profile, .. }
            | Feature::Revolve { profile, .. } => vec![*profile],
            Feature::Sweep { profile, path } => vec![*profile, *path],
            Feature::Loft { profiles } => profiles.clone(),
            Feature::Fillet { edges, .. } | Feature::Chamfer { edges, .. } => edges.clone(),
            Feature::InlineProfile(_) => Vec::new(),
        }
    }

    /// Human-readable type label — for status bars, undo stacks,
    /// debug logs.
    pub fn label(&self) -> &'static str {
        match self {
            Feature::Extrude { .. } => "Extrude",
            Feature::Cut { .. } => "Cut",
            Feature::Revolve { .. } => "Revolve",
            Feature::Sweep { .. } => "Sweep",
            Feature::Loft { .. } => "Loft",
            Feature::Fillet { .. } => "Fillet",
            Feature::Chamfer { .. } => "Chamfer",
            Feature::InlineProfile(_) => "Profile",
        }
    }
}

/// Ordered feature tree + dependency graph. The tree owns the
/// features; the graph owns reachability between them.
#[derive(Default)]
pub struct FeatureTree {
    /// Features in user-visible order. Insertion order is preserved
    /// (the graph decides *recompute* order separately).
    pub features: Vec<(EntityId, Feature)>,
    /// Dependency graph over the features in the tree.
    pub graph:    DependencyGraph,
}

impl FeatureTree {
    /// Empty tree.
    pub fn new() -> Self {
        Self::default()
    }

    /// Append a feature, populating the dependency graph from the
    /// feature's [`Feature::dependencies`]. Returns `Err` if any
    /// declared dependency is missing from the graph.
    pub fn push(
        &mut self,
        id: EntityId,
        feature: Feature,
    ) -> Result<(), super::core::GraphError> {
        let deps = feature.dependencies();
        self.graph.insert(Node::new(id));
        for dep in &deps {
            self.graph.add_dependency(id, *dep)?;
        }
        self.features.push((id, feature));
        Ok(())
    }

    /// Remove a feature by id. Also removes it from the graph.
    pub fn remove(&mut self, id: EntityId) -> Option<Feature> {
        self.graph.remove(id);
        if let Some(pos) = self.features.iter().position(|(fid, _)| *fid == id) {
            Some(self.features.remove(pos).1)
        } else {
            None
        }
    }

    /// Recompute order — feature ids in dependency-first order.
    pub fn recompute_order(&self) -> Result<Vec<EntityId>, super::core::GraphError> {
        self.graph.topological_order()
    }

    /// Feature ids that must re-evaluate when `id` changes.
    pub fn downstream_of(&self, id: EntityId) -> Vec<EntityId> {
        self.graph.downstream_of(id)
    }

    /// Look up a feature by id.
    pub fn get(&self, id: EntityId) -> Option<&Feature> {
        self.features
            .iter()
            .find_map(|(fid, feat)| if *fid == id { Some(feat) } else { None })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u64) -> EntityId {
        EntityId(n)
    }

    #[test]
    fn extrude_records_profile_dependency() {
        let f = Feature::Extrude {
            profile:   id(7),
            distance:  10.0,
            symmetric: false,
        };
        assert_eq!(f.dependencies(), vec![id(7)]);
    }

    #[test]
    fn feature_tree_tracks_dependencies() {
        let mut tree = FeatureTree::new();
        tree.push(id(1), Feature::InlineProfile(Profile { points: vec![] }))
            .unwrap();
        tree.push(
            id(2),
            Feature::Extrude {
                profile:   id(1),
                distance:  5.0,
                symmetric: false,
            },
        )
        .unwrap();
        let order = tree.recompute_order().unwrap();
        let pos = |t: EntityId| order.iter().position(|x| *x == t).unwrap();
        assert!(pos(id(1)) < pos(id(2)));
    }

    #[test]
    fn feature_tree_reports_downstream() {
        let mut tree = FeatureTree::new();
        tree.push(id(1), Feature::InlineProfile(Profile { points: vec![] }))
            .unwrap();
        tree.push(
            id(2),
            Feature::Extrude {
                profile:   id(1),
                distance:  5.0,
                symmetric: false,
            },
        )
        .unwrap();
        assert_eq!(tree.downstream_of(id(1)), vec![id(2)]);
    }
}
