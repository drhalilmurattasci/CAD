//! Entity-graph foundation for the CAD stack.
//!
//! Everything parametric eventually boils down to "this feature
//! depends on that sketch, which depends on those constraints" — a
//! directed acyclic graph of entities. This module owns the typed
//! ids, the generic [`Node`] record, and the [`DependencyGraph`] that
//! the parametric / feature-tree layers build on top.
//!
//! Deliberately kept free of any CAD-specific shape: a dependency
//! graph here could just as well track asset imports, script
//! references, or undo receipts. The parametric layer
//! ([`crate::cad::parametric`]) is what marries it to features.

use std::collections::{HashMap, HashSet, VecDeque};

use thiserror::Error;

use crate::id::IdAllocator;

/// Universal id for any CAD entity (sketch element, feature, B-Rep
/// face, …). Newtype over a `u64` so [`IdAllocator`] can hand them
/// out without each domain having to spin its own counter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct EntityId(pub u64);

impl From<u64> for EntityId {
    fn from(value: u64) -> Self {
        Self(value)
    }
}

impl std::fmt::Display for EntityId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "#{}", self.0)
    }
}

/// Default allocator for fresh [`EntityId`]s. Monotonic, never
/// reuses values — see [`crate::id`] for semantics.
pub type EntityIdAllocator = IdAllocator<EntityId>;

/// A node in the dependency graph: the id of the entity plus the set
/// of entity ids it directly depends on.
///
/// Keeping the payload-free form here is intentional. Rich data
/// (the actual sketch element, feature, etc.) lives in the owning
/// module; the graph only cares about reachability.
#[derive(Debug, Clone, Default)]
pub struct Node {
    /// Identity of the entity this node represents.
    pub id:           EntityId,
    /// Direct dependencies — entities that must recompute *before*
    /// this one.
    pub dependencies: Vec<EntityId>,
}

impl Node {
    /// Construct an isolated node with no dependencies.
    pub fn new(id: EntityId) -> Self {
        Self {
            id,
            dependencies: Vec::new(),
        }
    }

    /// Construct a node with an initial set of dependencies.
    pub fn with_dependencies(id: EntityId, dependencies: Vec<EntityId>) -> Self {
        Self { id, dependencies }
    }
}

/// Failure modes for graph operations.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum GraphError {
    /// The caller referenced an entity id that isn't in the graph.
    #[error("entity {0} is not in the graph")]
    UnknownEntity(EntityId),
    /// Adding an edge would introduce a cycle.
    #[error("cycle detected: edge {from} -> {to} would loop back")]
    Cycle {
        /// The node the edge originates from.
        from: EntityId,
        /// The node the edge would point to.
        to:   EntityId,
    },
}

/// Directed-acyclic dependency graph over [`EntityId`]s.
///
/// The graph owns the adjacency; nodes and the entities they refer
/// to live wherever the caller stores them. Operations are written
/// for modest graphs (hundreds to low-thousands of entities), which
/// is the typical CAD feature-tree size.
#[derive(Debug, Default, Clone)]
pub struct DependencyGraph {
    nodes: HashMap<EntityId, Node>,
}

impl DependencyGraph {
    /// Empty graph.
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert or replace a node. Does not check for cycles — use
    /// [`add_dependency`](Self::add_dependency) for safe edge insertion.
    pub fn insert(&mut self, node: Node) {
        self.nodes.insert(node.id, node);
    }

    /// Remove an entity and every edge that references it. Returns
    /// the removed node if it existed.
    pub fn remove(&mut self, id: EntityId) -> Option<Node> {
        let removed = self.nodes.remove(&id);
        for node in self.nodes.values_mut() {
            node.dependencies.retain(|dep| *dep != id);
        }
        removed
    }

    /// Look up a node by id.
    pub fn get(&self, id: EntityId) -> Option<&Node> {
        self.nodes.get(&id)
    }

    /// Iterator over every node in the graph. Order is
    /// implementation-defined (not topological).
    pub fn nodes(&self) -> impl Iterator<Item = &Node> {
        self.nodes.values()
    }

    /// Number of nodes in the graph.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// `true` when the graph has no nodes.
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Add an `a -> b` dependency edge ("`a` depends on `b`"). Fails
    /// if either endpoint is missing or if the edge would close a
    /// cycle.
    pub fn add_dependency(&mut self, a: EntityId, b: EntityId) -> Result<(), GraphError> {
        if !self.nodes.contains_key(&a) {
            return Err(GraphError::UnknownEntity(a));
        }
        if !self.nodes.contains_key(&b) {
            return Err(GraphError::UnknownEntity(b));
        }
        if self.reachable(b, a) {
            return Err(GraphError::Cycle { from: a, to: b });
        }
        let node = self.nodes.get_mut(&a).expect("node exists");
        if !node.dependencies.contains(&b) {
            node.dependencies.push(b);
        }
        Ok(())
    }

    /// `true` if `from` can reach `to` by following forward edges.
    pub fn reachable(&self, from: EntityId, to: EntityId) -> bool {
        if from == to {
            return true;
        }
        let mut stack = vec![from];
        let mut seen = HashSet::new();
        while let Some(id) = stack.pop() {
            if !seen.insert(id) {
                continue;
            }
            let Some(node) = self.nodes.get(&id) else {
                continue;
            };
            for dep in &node.dependencies {
                if *dep == to {
                    return true;
                }
                stack.push(*dep);
            }
        }
        false
    }

    /// Kahn-style topological sort. Returns ids in the order they
    /// must be evaluated — dependencies first, dependents after.
    /// Errors if the graph contains a cycle (shouldn't happen if
    /// every edge went through [`add_dependency`](Self::add_dependency)).
    pub fn topological_order(&self) -> Result<Vec<EntityId>, GraphError> {
        let mut incoming: HashMap<EntityId, usize> =
            self.nodes.keys().map(|id| (*id, 0usize)).collect();
        for node in self.nodes.values() {
            for dep in &node.dependencies {
                *incoming.entry(*dep).or_insert(0) += 1;
            }
        }
        let mut queue: VecDeque<EntityId> = incoming
            .iter()
            .filter_map(|(id, count)| if *count == 0 { Some(*id) } else { None })
            .collect();
        let mut out = Vec::with_capacity(self.nodes.len());
        while let Some(id) = queue.pop_front() {
            out.push(id);
            if let Some(node) = self.nodes.get(&id) {
                for dep in &node.dependencies {
                    let count = incoming.entry(*dep).or_insert(0);
                    if *count > 0 {
                        *count -= 1;
                        if *count == 0 {
                            queue.push_back(*dep);
                        }
                    }
                }
            }
        }
        if out.len() != self.nodes.len() {
            let leftover = self
                .nodes
                .keys()
                .copied()
                .find(|id| !out.contains(id))
                .unwrap_or_default();
            return Err(GraphError::Cycle {
                from: leftover,
                to:   leftover,
            });
        }
        // The Kahn traversal above yields dependencies-after-dependents
        // order (we peeled off leaves first). Flip it so that the
        // caller gets dependencies-first, which is the right order for
        // recompute.
        out.reverse();
        Ok(out)
    }

    /// Every entity that transitively depends on `id` — the set that
    /// needs to be re-evaluated when `id` changes.
    pub fn downstream_of(&self, id: EntityId) -> Vec<EntityId> {
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        let mut stack = vec![id];
        while let Some(cur) = stack.pop() {
            for node in self.nodes.values() {
                if node.dependencies.contains(&cur) && seen.insert(node.id) {
                    out.push(node.id);
                    stack.push(node.id);
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u64) -> EntityId {
        EntityId(n)
    }

    fn build() -> DependencyGraph {
        let mut g = DependencyGraph::new();
        g.insert(Node::new(id(1)));
        g.insert(Node::new(id(2)));
        g.insert(Node::new(id(3)));
        g.insert(Node::new(id(4)));
        // 1 → 2 → 4, 1 → 3 → 4
        g.add_dependency(id(2), id(1)).unwrap();
        g.add_dependency(id(3), id(1)).unwrap();
        g.add_dependency(id(4), id(2)).unwrap();
        g.add_dependency(id(4), id(3)).unwrap();
        g
    }

    #[test]
    fn rejects_cycles() {
        let mut g = build();
        let err = g.add_dependency(id(1), id(4)).unwrap_err();
        assert!(matches!(err, GraphError::Cycle { .. }));
    }

    #[test]
    fn topological_order_respects_edges() {
        let g = build();
        let order = g.topological_order().unwrap();
        let pos = |t: EntityId| order.iter().position(|x| *x == t).unwrap();
        assert!(pos(id(1)) < pos(id(2)));
        assert!(pos(id(1)) < pos(id(3)));
        assert!(pos(id(2)) < pos(id(4)));
        assert!(pos(id(3)) < pos(id(4)));
    }

    #[test]
    fn downstream_includes_all_transitive_dependents() {
        let g = build();
        let mut downstream = g.downstream_of(id(1));
        downstream.sort();
        assert_eq!(downstream, vec![id(2), id(3), id(4)]);
    }

    #[test]
    fn remove_drops_dangling_edges() {
        let mut g = build();
        g.remove(id(2));
        assert!(g.get(id(2)).is_none());
        // Node 4 used to depend on 2; should no longer.
        assert!(!g.get(id(4)).unwrap().dependencies.contains(&id(2)));
    }
}
