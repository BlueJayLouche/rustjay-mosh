use std::path::Path;

use image::ImageEncoder;
use rand::random;
use thiserror::Error;

use crate::codec::ir::Yuv420;
use crate::packet::{OwnedPacket, PacketClip};
use crate::preview::decoder::PacketDecoder;

#[derive(Debug, Error)]
pub enum BakeError {
    #[error("ffmpeg error: {0}")]
    Ffmpeg(#[from] ffmpeg_next::Error),
    #[error("decode error: {0}")]
    Decode(#[from] crate::preview::decoder::DecodeError),
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
    #[error("jpeg error: {0}")]
    Jpeg(String),
    #[error("import error: {0}")]
    Import(#[from] crate::importer::ImportError),
    #[error("no frames decoded")]
    NoFrames,
}

// ------------------------------------------------------------------
// Data bending
// ------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
pub enum BendMode {
    /// Reverse the byte order of every scanline in the Y plane.
    ReverseScanlines,
    /// Repeat rows with an offset delay, mixed at `mix` ratio.
    Echo { delay: usize, mix: f32 },
    /// Reduce each byte to `bits` significant bits (posterise).
    Bitcrush { bits: u8 },
    /// Swap every `stride` bytes.
    ByteSwap { stride: usize },
    /// XOR each byte with a mask.
    Xor { mask: u8 },
    /// Add random noise in range ±amount.
    Noise { amount: u8 },
}

/// Apply a databending effect to a decoded YUV420 frame in-place.
pub fn bend_yuv(yuv: &mut Yuv420, mode: BendMode) {
    match mode {
        BendMode::ReverseScanlines => {
            let w = yuv.width as usize;
            for row in yuv.y.chunks_exact_mut(w) {
                row.reverse();
            }
        }
        BendMode::Echo { delay, mix } => {
            let w = yuv.width as usize;
            let h = yuv.height as usize;
            if delay > 0 && delay < h {
                let original = yuv.y.clone();
                for row in 0..h {
                    let src_row = row.saturating_sub(delay);
                    for col in 0..w {
                        let idx = row * w + col;
                        let src_idx = src_row * w + col;
                        let a = original[idx] as f32;
                        let b = original[src_idx] as f32;
                        yuv.y[idx] = (a * (1.0 - mix) + b * mix).clamp(0.0, 255.0) as u8;
                    }
                }
            }
        }
        BendMode::Bitcrush { bits } => {
            let shift = 8u8.saturating_sub(bits.clamp(1, 8));
            let mask = !0u8 << shift;
            for b in &mut yuv.y {
                *b &= mask;
            }
            for b in &mut yuv.u {
                *b = (*b & mask) | 128;
            }
            for b in &mut yuv.v {
                *b = (*b & mask) | 128;
            }
        }
        BendMode::ByteSwap { stride } => {
            if stride > 1 {
                for plane in [&mut yuv.y, &mut yuv.u, &mut yuv.v] {
                    for chunk in plane.chunks_exact_mut(stride) {
                        if chunk.len() >= 2 {
                            chunk.swap(0, chunk.len() - 1);
                        }
                    }
                }
            }
        }
        BendMode::Xor { mask } => {
            for b in &mut yuv.y {
                *b ^= mask;
            }
            for b in &mut yuv.u {
                *b ^= mask;
            }
            for b in &mut yuv.v {
                *b ^= mask;
            }
        }
        BendMode::Noise { amount } => {
            let amt = amount as i16;
            for b in &mut yuv.y {
                let delta = random::<i16>().wrapping_rem_euclid(2 * amt + 1) - amt;
                *b = (*b as i16 + delta).clamp(0, 255) as u8;
            }
            for b in &mut yuv.u {
                let delta = random::<i16>().wrapping_rem_euclid(2 * amt + 1) - amt;
                *b = (*b as i16 + delta).clamp(0, 255) as u8;
            }
            for b in &mut yuv.v {
                let delta = random::<i16>().wrapping_rem_euclid(2 * amt + 1) - amt;
                *b = (*b as i16 + delta).clamp(0, 255) as u8;
            }
        }
    }
}

// ------------------------------------------------------------------
// Compression artifacting (JPEG re-compress region)
// ------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
pub struct CompressRegion {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
    /// 1 – 100, lower = more artifacts.
    pub quality: u8,
}

/// Re-compress a rectangular region of a YUV420 frame through JPEG at low
/// quality, producing authentic macroblock/DCT artifacts.
pub fn compress_region(yuv: &mut Yuv420, region: &CompressRegion) -> Result<(), BakeError> {
    let (fw, fh) = (yuv.width, yuv.height);
    let x = region.x.min(fw - 1);
    let y = region.y.min(fh - 1);
    let w = region.w.min(fw - x);
    let h = region.h.min(fh - y);
    if w == 0 || h == 0 {
        return Ok(());
    }

    // Extract region → RGB
    let mut rgb = Vec::with_capacity((w * h * 3) as usize);
    for row in y..y + h {
        for col in x..x + w {
            let (r, g, b) = yuv_pixel_to_rgb(yuv, col, row);
            rgb.extend_from_slice(&[r, g, b]);
        }
    }

    // Encode RGB → JPEG
    let quality = region.quality.clamp(1, 100);
    let mut jpeg_bytes = Vec::new();
    {
        let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg_bytes, quality);
        encoder
            .write_image(&rgb, w, h, image::ColorType::Rgb8.into())
            .map_err(|e| BakeError::Jpeg(format!("{e}")))?;
    }

    // Decode JPEG → RGB
    let decoded = image::load_from_memory(&jpeg_bytes)
        .map_err(|e| BakeError::Jpeg(format!("{e}")))?
        .to_rgb8();

    // Paste decoded RGB back into YUV region
    for row in 0..h {
        for col in 0..w {
            let src_idx = ((row * w + col) * 3) as usize;
            let pixel = decoded.get(src_idx..src_idx + 3).unwrap_or(&[0, 0, 0]);
            let (yy, uu, vv) = rgb_to_yuv(pixel[0], pixel[1], pixel[2]);
            set_yuv_pixel(yuv, x + col, y + row, yy, uu, vv);
        }
    }

    Ok(())
}

fn yuv_pixel_to_rgb(yuv: &Yuv420, x: u32, y: u32) -> (u8, u8, u8) {
    let y_val = yuv.y[(y * yuv.width + x) as usize] as f32;
    let u_val = yuv.u[((y / 2) * yuv.chroma_width() + (x / 2)) as usize] as f32;
    let v_val = yuv.v[((y / 2) * yuv.chroma_width() + (x / 2)) as usize] as f32;

    let r = (y_val + 1.402 * (v_val - 128.0)).clamp(0.0, 255.0) as u8;
    let g = (y_val - 0.344136 * (u_val - 128.0) - 0.714136 * (v_val - 128.0)).clamp(0.0, 255.0) as u8;
    let b = (y_val + 1.772 * (u_val - 128.0)).clamp(0.0, 255.0) as u8;
    (r, g, b)
}

fn rgb_to_yuv(r: u8, g: u8, b: u8) -> (u8, u8, u8) {
    let r = r as f32;
    let g = g as f32;
    let b = b as f32;

    let y = (0.299 * r + 0.587 * g + 0.114 * b).clamp(0.0, 255.0) as u8;
    let u = (-0.168736 * r - 0.331264 * g + 0.5 * b + 128.0).clamp(0.0, 255.0) as u8;
    let v = (0.5 * r - 0.418688 * g - 0.081312 * b + 128.0).clamp(0.0, 255.0) as u8;
    (y, u, v)
}

fn set_yuv_pixel(yuv: &mut Yuv420, x: u32, y: u32, yy: u8, uu: u8, vv: u8) {
    let cw = yuv.chroma_width();
    yuv.y[(y * yuv.width + x) as usize] = yy;
    yuv.u[((y / 2) * cw + (x / 2)) as usize] = uu;
    yuv.v[((y / 2) * cw + (x / 2)) as usize] = vv;
}

// ------------------------------------------------------------------
// YUV → RGBA helper (for egui dialog preview)
// ------------------------------------------------------------------

pub fn yuv_to_rgba(yuv: &Yuv420) -> Vec<u8> {
    let mut rgba = Vec::with_capacity((yuv.width * yuv.height * 4) as usize);
    for y in 0..yuv.height {
        for x in 0..yuv.width {
            let (r, g, b) = yuv_pixel_to_rgb(yuv, x, y);
            rgba.extend_from_slice(&[r, g, b, 255]);
        }
    }
    rgba
}

// ------------------------------------------------------------------
// Bake orchestration
// ------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum Effect {
    Bend(BendMode),
    Compress(CompressRegion),
}

/// Decode a segment of a clip, apply an effect, re-encode to H.264, and
/// return a new `PacketClip`.
pub fn bake_segment(
    source_clip: &PacketClip,
    start_frame: usize,
    frame_count: usize,
    effect: Effect,
    fps: u32,
) -> Result<PacketClip, BakeError> {
    let total = source_clip.packets.len();
    let end = (start_frame + frame_count).min(total);
    let actual_count = end.saturating_sub(start_frame);
    if actual_count == 0 {
        return Err(BakeError::NoFrames);
    }

    // 1. Decode frames sequentially.
    let mut decoder = PacketDecoder::new(&source_clip.codec_parameters)?;
    let packet_refs: Vec<&OwnedPacket> = source_clip.packets.iter().collect();
    let all_frames = decoder.decode_all(&packet_refs)?;

    let mut frames: Vec<Yuv420> = all_frames
        .into_iter()
        .skip(start_frame)
        .take(actual_count)
        .map(|arc| (*arc).clone())
        .collect();

    if frames.is_empty() {
        return Err(BakeError::NoFrames);
    }

    // 2. Apply effect per frame.
    for yuv in &mut frames {
        match &effect {
            Effect::Bend(mode) => bend_yuv(yuv, *mode),
            Effect::Compress(region) => compress_region(yuv, region)?,
        }
    }

    // 3. Write YUV sequence → raw file → H.264 via ffmpeg.
    let temp_dir = tempfile::tempdir()?;
    let raw_path = temp_dir.path().join("frames.yuv");
    let mp4_path = temp_dir.path().join("baked.mp4");

    write_yuv_sequence(&frames, &raw_path)?;
    encode_raw_yuv_to_h264(&raw_path, &mp4_path, fps, frames[0].width, frames[0].height)?;

    // 4. Import the baked MP4 as a new PacketClip.
    let (clip, _first_yuv) = crate::importer::import_video(&mp4_path, "baked")?;
    Ok(clip)
}

fn write_yuv_sequence(frames: &[Yuv420], path: &Path) -> Result<(), std::io::Error> {
    use std::io::Write;
    let mut file = std::fs::File::create(path)?;
    for yuv in frames {
        file.write_all(&yuv.y)?;
        file.write_all(&yuv.u)?;
        file.write_all(&yuv.v)?;
    }
    Ok(())
}

fn encode_raw_yuv_to_h264(
    raw_path: &Path,
    output: &Path,
    fps: u32,
    width: u32,
    height: u32,
) -> Result<(), BakeError> {
    let status = std::process::Command::new(crate::bundled_ffmpeg())
        .args([
            "-y",
            "-f", "rawvideo",
            "-pix_fmt", "yuv420p",
            "-s", &format!("{}x{}", width, height),
            "-r", &fps.to_string(),
            "-i", raw_path.to_str().unwrap_or(""),
            "-c:v", "libx264",
            "-preset", "fast",
            "-crf", "18",
            "-pix_fmt", "yuv420p",
            "-movflags", "faststart",
            output.to_str().unwrap_or(""),
        ])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()?;

    if !status.success() {
        return Err(BakeError::Ffmpeg(ffmpeg_next::Error::Other { errno: 1 }));
    }
    Ok(())
}
