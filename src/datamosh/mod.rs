use crate::frame_graph::{FrameGraph, FrameId};

/// Remove all I-frames in `asset_id` from the graph so that P-frames
/// reference across what were formerly intra boundaries, producing the
/// classic "datamosh smear" effect.
pub fn remove_iframes(graph: &mut FrameGraph, iframes: &[usize]) {
    // Collect children of each I-frame and rewire them to the I-frame's own
    // parent, collapsing the I-frame out of the reference chain.
    for &iframe_node in iframes {
        let parent = graph.reference_of(iframe_node);
        // Find every node that directly references this I-frame.
        let dependents: Vec<usize> = (0..graph.len())
            .filter(|&n| graph.reference_of(n) == Some(iframe_node))
            .collect();
        for dep in dependents {
            match parent {
                Some(p) => graph.rewire(dep, p),
                None => graph.remove_reference(dep),
            }
        }
    }
}

/// Rewire a range of frames in `dst_asset` to reference frames from
/// `src_asset`, causing motion vectors to be interpreted against the
/// wrong content — the canonical cross-clip datamosh.
pub fn cross_clip_mosh(
    graph: &mut FrameGraph,
    dst_frames: &[FrameId],
    src_frames: &[FrameId],
) {
    for (dst, src) in dst_frames.iter().zip(src_frames.iter()) {
        if let (Some(dst_node), Some(src_node)) =
            (graph.node_index(*dst), graph.node_index(*src))
        {
            graph.rewire(dst_node, src_node);
        }
    }
}
