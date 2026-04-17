use std::io::Write as _;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Arc;

use crate::codec::decoder::decode_frame;
use crate::codec::ir::{Frame, Yuv420};
use crate::format::binary::FormatError;

#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    #[error("decode error: {0}")]
    Decode(#[from] FormatError),
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
    #[error("no frames to render")]
    NoFrames,
}

/// Decode `frame_indices` (in timeline order) from `frame_store` using the
/// cache, then pipe raw YUV420 to an `ffmpeg` subprocess encoding to `output_path`.
///
/// `decode_cache` is updated in place so the caller retains hot entries.
pub fn export_video(
    frame_indices: &[usize],
    frame_store: &[Frame],
    decode_cache: &mut Vec<Option<Arc<Yuv420>>>,
    output_path: &Path,
    fps: u32,
) -> Result<(), ExportError> {
    // Decode the first frame to get dimensions.
    let first_yuv = decode_cached(frame_indices[0], frame_store, decode_cache)?;
    let (w, h) = (first_yuv.width, first_yuv.height);

    let mut child = Command::new("ffmpeg")
        .args([
            "-y",
            "-f", "rawvideo",
            "-pixel_format", "yuv420p",
            "-video_size", &format!("{w}x{h}"),
            "-framerate", &fps.to_string(),
            "-i", "pipe:0",
            "-c:v", "libx264",
            "-preset", "fast",
            "-crf", "18",
            "-pix_fmt", "yuv420p",
            output_path.to_str().unwrap_or("output.mp4"),
        ])
        .stdin(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;

    {
        let stdin = child.stdin.as_mut().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::BrokenPipe, "no stdin")
        })?;

        for &idx in frame_indices {
            let yuv = decode_cached(idx, frame_store, decode_cache)?;
            stdin.write_all(&yuv.y)?;
            stdin.write_all(&yuv.u)?;
            stdin.write_all(&yuv.v)?;
        }
    }

    child.wait()?;
    Ok(())
}

/// Decode a single frame from `frame_store`, using `cache` to avoid redundant work.
pub fn decode_cached(
    idx: usize,
    frame_store: &[Frame],
    cache: &mut Vec<Option<Arc<Yuv420>>>,
) -> Result<Arc<Yuv420>, ExportError> {
    if cache[idx].is_none() {
        let yuv = decode_frame(&frame_store[idx], frame_store)?;
        cache[idx] = Some(Arc::new(yuv));
    }
    Ok(cache[idx].clone().unwrap())
}
