use crate::codec::decoder::decode_frame;
use crate::codec::ir::{Frame, Yuv420};
use crate::format::binary::FormatError;

/// Fast preview: decode a single frame given the full frame store.
///
/// Does not perform any scaling or colour conversion — callers are
/// responsible for converting YUV420 to whatever surface format they need.
pub fn preview_frame(frame: &Frame, frame_store: &[Frame]) -> Result<Yuv420, FormatError> {
    decode_frame(frame, frame_store)
}
