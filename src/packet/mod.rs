/// Lightweight owned copy of an encoded video packet.
#[derive(Debug, Clone)]
pub struct OwnedPacket {
    pub data: Vec<u8>,
    pub pts: i64,
    pub dts: i64,
    pub duration: i64,
    pub is_key: bool,
}

use ffmpeg_next as ffmpeg;

/// A transcoded clip stored as raw H.264 packets.
/// All clips are normalized to the same resolution during import.
pub struct PacketClip {
    pub id: u64,
    pub name: String,
    pub packets: Vec<OwnedPacket>,
    pub width: u32,
    pub height: u32,
    pub codec_parameters: ffmpeg::codec::Parameters,
    pub time_base: ffmpeg::Rational,
}

impl std::fmt::Debug for PacketClip {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PacketClip")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("packets", &self.packets.len())
            .field("width", &self.width)
            .field("height", &self.height)
            .field("time_base", &self.time_base)
            .finish_non_exhaustive()
    }
}

impl PacketClip {
    /// Return the indices of keyframe packets.
    pub fn keyframe_indices(&self) -> Vec<usize> {
        self.packets
            .iter()
            .enumerate()
            .filter(|(_, p)| p.is_key)
            .map(|(i, _)| i)
            .collect()
    }
}

/// Build the effective flat packet sequence for a contiguous clip span.
/// `drop_leading_keyframe` indicates whether the first *visible* keyframe of
/// this span should be dropped so that the decoder state bleeds in from the
/// preceding clip.
#[derive(Debug, Clone)]
pub struct ClipSpan<'a> {
    pub clip: &'a PacketClip,
    /// Number of source packets to skip from the start of the clip.
    pub source_offset: usize,
    /// How many source packets to include after the offset.
    pub visible_count: usize,
    /// If true, skip the first visible keyframe and start from the first P-frame.
    pub drop_leading_keyframe: bool,
}

impl<'a> ClipSpan<'a> {
    pub fn iter_packets(&self) -> impl Iterator<Item = &'a OwnedPacket> + use<'a> {
        let start = self.source_offset + if self.drop_leading_keyframe { 1 } else { 0 };
        self.clip.packets.iter().skip(start).take(self.visible_count)
    }

    pub fn packet_count(&self) -> usize {
        self.visible_count
            .min(self.clip.packets.len().saturating_sub(self.source_offset))
    }
}

/// Resolve a timeline of ordered clips into a single flat `Vec` of packet references.
pub fn build_sequence<'a>(spans: &[ClipSpan<'a>]) -> Vec<&'a OwnedPacket> {
    let mut out = Vec::new();
    for span in spans {
        out.extend(span.iter_packets());
    }
    out
}
