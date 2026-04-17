use std::collections::HashMap;

/// Identifies a frame within a specific asset in the media pool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FrameId {
    pub asset_id: u32,
    pub frame_idx: u32,
}

/// Directed graph where each node is a frame and each edge is a P-frame
/// prediction dependency (child → parent). I-frames have no parent edge.
///
/// Graph rewiring (datamoshing) is achieved by changing a node's parent
/// to a frame from a different clip, causing motion vectors to decode
/// against unintended reference data.
#[derive(Debug, Default)]
pub struct FrameGraph {
    /// All nodes in insertion order; index == node id used in edges.
    nodes: Vec<FrameId>,
    /// Maps node index → parent node index.
    edges: HashMap<usize, usize>,
    /// Reverse index: FrameId → node index.
    index: HashMap<FrameId, usize>,
}

impl FrameGraph {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a frame node. Returns the new node's index.
    pub fn add_node(&mut self, id: FrameId) -> usize {
        let idx = self.nodes.len();
        self.nodes.push(id);
        self.index.insert(id, idx);
        idx
    }

    /// Set a prediction dependency: `child` references `parent`.
    pub fn set_reference(&mut self, child: usize, parent: usize) {
        self.edges.insert(child, parent);
    }

    /// Remove the reference edge from `child`, making it an implicit I-frame root.
    pub fn remove_reference(&mut self, child: usize) {
        self.edges.remove(&child);
    }

    /// Rewire `child` to reference a different `new_parent`.
    /// This is the core datamosh operation — motion bleeds across clips.
    pub fn rewire(&mut self, child: usize, new_parent: usize) {
        self.edges.insert(child, new_parent);
    }

    pub fn reference_of(&self, node: usize) -> Option<usize> {
        self.edges.get(&node).copied()
    }

    pub fn node(&self, idx: usize) -> Option<FrameId> {
        self.nodes.get(idx).copied()
    }

    pub fn node_index(&self, id: FrameId) -> Option<usize> {
        self.index.get(&id).copied()
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }
}
