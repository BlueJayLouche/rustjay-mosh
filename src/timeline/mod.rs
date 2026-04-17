use crate::frame_graph::FrameId;

/// A contiguous span of frames placed on the timeline.
#[derive(Debug, Clone)]
pub struct Clip {
    /// Start position on the timeline (in frames).
    pub timeline_start: u64,
    /// Ordered sequence of frame ids that make up this clip.
    pub frames: Vec<FrameId>,
}

impl Clip {
    pub fn duration(&self) -> u64 {
        self.frames.len() as u64
    }

    pub fn timeline_end(&self) -> u64 {
        self.timeline_start + self.duration()
    }
}

/// The NLE timeline: an ordered collection of clips.
#[derive(Debug, Default)]
pub struct Timeline {
    clips: Vec<Clip>,
}

impl Timeline {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_clip(&mut self, clip: Clip) {
        self.clips.push(clip);
        self.clips.sort_by_key(|c| c.timeline_start);
    }

    /// Return the frame id at the given timeline position, if any.
    pub fn frame_at(&self, position: u64) -> Option<FrameId> {
        for clip in &self.clips {
            if position >= clip.timeline_start && position < clip.timeline_end() {
                let local = (position - clip.timeline_start) as usize;
                return clip.frames.get(local).copied();
            }
        }
        None
    }

    pub fn clips(&self) -> &[Clip] {
        &self.clips
    }

    pub fn duration(&self) -> u64 {
        self.clips.iter().map(|c| c.timeline_end()).max().unwrap_or(0)
    }
}
