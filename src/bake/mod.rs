use std::sync::atomic::{AtomicBool, Ordering};

use ffmpeg_next as ffmpeg;
use image::ImageEncoder;
use rand::{Rng, SeedableRng};
use rand::rngs::SmallRng;
use thiserror::Error;

use crate::codec::ir::Yuv420;
use crate::packet::{OwnedPacket, PacketClip};
use crate::preview::decoder::PacketDecoder;

#[derive(Debug, Error)]
pub enum BakeError {
    #[error("ffmpeg error: {0}")]
    Ffmpeg(#[from] ffmpeg::Error),
    #[error("decode error: {0}")]
    Decode(#[from] crate::preview::decoder::DecodeError),
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
    #[error("jpeg error: {0}")]
    Jpeg(String),
    #[error("encode error: {0}")]
    Encode(String),
    #[error(
        "region ({x},{y} {w}×{h}) out of bounds for frame {fw}×{fh}"
    )]
    InvalidRegion {
        fw: u32,
        fh: u32,
        x: u32,
        y: u32,
        w: u32,
        h: u32,
    },
    #[error("no frames decoded")]
    NoFrames,
    #[error("cancelled")]
    Cancelled,
}

// ------------------------------------------------------------------
// Data bending
// ------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BendMode {
    /// Reverse the byte order of every scanline in the Y plane.
    ReverseScanlines,
    /// Repeat rows with an offset delay, mixed at `mix` ratio.
    Echo { delay: usize, mix: f32 },
    /// Reduce each byte to `bits` significant bits (posterise) on all planes.
    Bitcrush { bits: u8 },
    /// Reverse every `stride`-byte chunk of each plane.
    ByteSwap { stride: usize },
    /// XOR each byte with a mask.
    Xor { mask: u8 },
    /// Add deterministic pseudo-random noise in range ±amount, seeded by `seed`.
    Noise { amount: u8, seed: u64 },
    /// Asendorf-style pixel sorting: sort runs of pixels whose key value lies
    /// in `lo..=hi`, along `dir`, descending when `reverse`.
    PixelSort { dir: SortDir, key: SortKey, lo: u8, hi: u8, reverse: bool },
    /// Oscillating edge ghosts (analog-TV / over-sharpened look) on the Y
    /// plane, horizontal. `amount` is echo strength, `period` the ghost
    /// spacing in pixels.
    Ringing { amount: f32, period: usize },
    /// Shift the U plane by (dx, dy) and the V plane by (-dx, -dy) —
    /// chromatic-aberration colour bleed. Offsets are in chroma pixels.
    /// `wrap` rolls pixels around the frame edge; otherwise edges clamp
    /// (lens-style aberration).
    ChromaShift { dx: i32, dy: i32, wrap: bool },
}

impl BendMode {
    /// Per-frame variant of this mode: noise reseeds each frame so grain
    /// animates instead of sticking to the screen like a dirty pane.
    pub fn for_frame(self, frame: u64) -> Self {
        match self {
            BendMode::Noise { amount, seed } => BendMode::Noise {
                amount,
                seed: seed.wrapping_add(frame),
            },
            m => m,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortDir {
    Horizontal,
    Vertical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortKey {
    Luma,
    Hue,
    Saturation,
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
            for plane in [&mut yuv.y, &mut yuv.u, &mut yuv.v] {
                for b in plane.iter_mut() {
                    *b &= mask;
                }
            }
        }
        BendMode::ByteSwap { stride } => {
            if stride > 1 {
                for plane in [&mut yuv.y, &mut yuv.u, &mut yuv.v] {
                    for chunk in plane.chunks_mut(stride) {
                        chunk.reverse();
                    }
                }
            }
        }
        BendMode::Xor { mask } => {
            for plane in [&mut yuv.y, &mut yuv.u, &mut yuv.v] {
                for b in plane.iter_mut() {
                    *b ^= mask;
                }
            }
        }
        BendMode::Noise { amount, seed } => {
            let amt = amount as i16;
            let range = 2 * amt + 1;
            let mut rng = SmallRng::seed_from_u64(seed);
            for plane in [&mut yuv.y, &mut yuv.u, &mut yuv.v] {
                for b in plane.iter_mut() {
                    let delta = rng.gen_range(0..range) - amt;
                    *b = (*b as i16 + delta).clamp(0, 255) as u8;
                }
            }
        }
        BendMode::PixelSort { dir, key, lo, hi, reverse } => {
            pixel_sort(yuv, dir, key, lo, hi, reverse);
        }
        BendMode::Ringing { amount, period } => {
            ringing(yuv, amount, period.max(1));
        }
        BendMode::ChromaShift { dx, dy, wrap } => {
            let cw = yuv.chroma_width() as usize;
            let ch = yuv.chroma_height() as usize;
            shift_plane(&mut yuv.u, cw, ch, dx, dy, wrap);
            shift_plane(&mut yuv.v, cw, ch, -dx, -dy, wrap);
        }
    }
}

/// Sort value 0-255 for an RGB pixel under the given key.
fn sort_key_value(key: SortKey, r: u8, g: u8, b: u8) -> u8 {
    match key {
        SortKey::Luma => (0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32) as u8,
        SortKey::Hue => {
            let (max, min) = (r.max(g).max(b) as f32, r.min(g).min(b) as f32);
            let d = max - min;
            if d == 0.0 {
                return 0;
            }
            let (r, g, b) = (r as f32, g as f32, b as f32);
            let h = if max == r {
                ((g - b) / d).rem_euclid(6.0)
            } else if max == g {
                (b - r) / d + 2.0
            } else {
                (r - g) / d + 4.0
            };
            (h / 6.0 * 255.0) as u8
        }
        SortKey::Saturation => {
            let (max, min) = (r.max(g).max(b), r.min(g).min(b));
            if max == 0 { 0 } else { ((max - min) as u32 * 255 / max as u32) as u8 }
        }
    }
}

fn pixel_sort(yuv: &mut Yuv420, dir: SortDir, key: SortKey, lo: u8, hi: u8, reverse: bool) {
    let (lines, len) = match dir {
        SortDir::Horizontal => (yuv.height, yuv.width),
        SortDir::Vertical => (yuv.width, yuv.height),
    };
    let coord = |line: u32, i: u32| match dir {
        SortDir::Horizontal => (i, line),
        SortDir::Vertical => (line, i),
    };
    let (lo, hi) = (lo.min(hi), lo.max(hi));

    for line in 0..lines {
        let px: Vec<(u8, u8, u8)> = (0..len)
            .map(|i| {
                let (x, y) = coord(line, i);
                yuv_pixel_to_rgb(yuv, x, y)
            })
            .collect();
        let keys: Vec<u8> = px.iter().map(|&(r, g, b)| sort_key_value(key, r, g, b)).collect();

        // Find maximal runs whose key lies in the threshold band and sort each.
        let mut i = 0usize;
        while i < len as usize {
            if keys[i] < lo || keys[i] > hi {
                i += 1;
                continue;
            }
            let start = i;
            while i < len as usize && keys[i] >= lo && keys[i] <= hi {
                i += 1;
            }
            if i - start < 2 {
                continue;
            }
            let mut run: Vec<usize> = (start..i).collect();
            run.sort_by_key(|&j| keys[j]);
            if reverse {
                run.reverse();
            }
            for (offset, &src) in run.iter().enumerate() {
                let (x, y) = coord(line, (start + offset) as u32);
                let (r, g, b) = px[src];
                let (yy, uu, vv) = rgb_to_yuv(r, g, b);
                set_yuv_pixel(yuv, x, y, yy, uu, vv);
            }
        }
    }
}

fn ringing(yuv: &mut Yuv420, amount: f32, period: usize) {
    let w = yuv.width as usize;
    // ponytail: fixed 3 alternating decaying taps — parametrise if anyone asks.
    const TAPS: usize = 3;
    const DECAY: f32 = 0.6;
    for row in yuv.y.chunks_exact_mut(w) {
        let orig: Vec<u8> = row.to_vec();
        for x in 0..w {
            let mut acc = orig[x] as f32;
            let mut gain = amount;
            for k in 1..=TAPS {
                let d = k * period;
                if x < d + 1 {
                    break;
                }
                // Edge (gradient) `d` pixels back, echoed with alternating sign.
                let edge = orig[x - d] as f32 - orig[x - d - 1] as f32;
                acc += if k % 2 == 1 { gain } else { -gain } * edge;
                gain *= DECAY;
            }
            row[x] = acc.clamp(0.0, 255.0) as u8;
        }
    }
}

fn shift_plane(plane: &mut [u8], w: usize, h: usize, dx: i32, dy: i32, wrap: bool) {
    let orig = plane.to_vec();
    for y in 0..h {
        for x in 0..w {
            let (sx, sy) = if wrap {
                (
                    (x as i32 - dx).rem_euclid(w as i32) as usize,
                    (y as i32 - dy).rem_euclid(h as i32) as usize,
                )
            } else {
                (
                    (x as i32 - dx).clamp(0, w as i32 - 1) as usize,
                    (y as i32 - dy).clamp(0, h as i32 - 1) as usize,
                )
            };
            plane[y * w + x] = orig[sy * w + sx];
        }
    }
}

// ------------------------------------------------------------------
// Compression artifacting (JPEG re-compress region)
// ------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq)]
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
///
/// Returns [`BakeError::InvalidRegion`] if the region origin lies outside the
/// frame or the region is empty. The region is clamped on the right/bottom
/// edges so requests that overflow the frame are still honoured.
pub fn compress_region(yuv: &mut Yuv420, region: &CompressRegion) -> Result<(), BakeError> {
    let (fw, fh) = (yuv.width, yuv.height);
    if region.w == 0 || region.h == 0 || region.x >= fw || region.y >= fh {
        return Err(BakeError::InvalidRegion {
            fw,
            fh,
            x: region.x,
            y: region.y,
            w: region.w,
            h: region.h,
        });
    }
    let x = region.x;
    let y = region.y;
    let w = region.w.min(fw - x);
    let h = region.h.min(fh - y);

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
///
/// `progress` is called with values in 0.0..=1.0 at stage boundaries and per
/// frame during the effect pass. `cancel` is checked between stages and per
/// frame; if it flips to `true`, the function returns [`BakeError::Cancelled`]
/// without producing output.
pub fn bake_segment(
    source_clip: &PacketClip,
    start_frame: usize,
    frame_count: usize,
    effect: Effect,
    fps: u32,
    progress: &mut dyn FnMut(f32),
    cancel: &AtomicBool,
) -> Result<PacketClip, BakeError> {
    let total = source_clip.packets.len();
    let end = (start_frame + frame_count).min(total);
    let actual_count = end.saturating_sub(start_frame);
    if actual_count == 0 {
        return Err(BakeError::NoFrames);
    }

    progress(0.0);
    if cancel.load(Ordering::Relaxed) { return Err(BakeError::Cancelled); }

    // 1. Decode just the requested range (seek to the nearest keyframe).
    let mut decoder = PacketDecoder::new(&source_clip.codec_parameters)?;
    let packet_refs: Vec<&OwnedPacket> = source_clip.packets.iter().collect();
    let decoded = decoder.decode_range(&packet_refs, start_frame, actual_count)?;

    let mut frames: Vec<Yuv420> = decoded.into_iter().map(|arc| (*arc).clone()).collect();

    if frames.is_empty() {
        return Err(BakeError::NoFrames);
    }

    progress(0.30);
    if cancel.load(Ordering::Relaxed) { return Err(BakeError::Cancelled); }

    // 2. Apply effect per frame.
    let frame_total = frames.len() as f32;
    for (i, yuv) in frames.iter_mut().enumerate() {
        match &effect {
            Effect::Bend(mode) => bend_yuv(yuv, mode.for_frame(i as u64)),
            Effect::Compress(region) => compress_region(yuv, region)?,
        }
        progress(0.30 + 0.40 * ((i as f32 + 1.0) / frame_total));
        if cancel.load(Ordering::Relaxed) { return Err(BakeError::Cancelled); }
    }

    progress(0.70);

    // 3. Encode in-process to a long-GOP no-B-frame H.264 PacketClip.
    //    Encode takes 0.70..=0.95, read-back takes 0.95..=1.0.
    let clip = encode_yuv_to_packet_clip(&frames, fps, progress, cancel)?;

    progress(1.0);
    Ok(clip)
}

/// Encode a sequence of YUV420 frames in-process to an H.264 `PacketClip`
/// configured with one leading keyframe + all P-frames (long-GOP, no B-frames)
/// so downstream datamosh operations see the usual I/P structure.
///
/// `progress` is driven over the 0.70..=0.95 band (encode) and 0.95..=1.0
/// (read-back). `cancel` is polled per input frame and per output packet.
fn encode_yuv_to_packet_clip(
    frames: &[Yuv420],
    fps: u32,
    progress: &mut dyn FnMut(f32),
    cancel: &AtomicBool,
) -> Result<PacketClip, BakeError> {
    let width = frames[0].width;
    let height = frames[0].height;

    let codec = ffmpeg::encoder::find(ffmpeg::codec::Id::H264)
        .ok_or_else(|| BakeError::Encode("libx264 encoder not available".into()))?;

    let tmp = tempfile::tempdir()?;
    let path = tmp.path().join("baked.mp4");

    let mut octx = ffmpeg::format::output(&path)?;
    let global_header = octx
        .format()
        .flags()
        .contains(ffmpeg::format::Flags::GLOBAL_HEADER);

    let enc_tb = ffmpeg::Rational(1, fps as i32);

    let mut video = ffmpeg::codec::context::Context::new_with_codec(codec)
        .encoder()
        .video()?;
    video.set_width(width);
    video.set_height(height);
    video.set_format(ffmpeg::util::format::Pixel::YUV420P);
    video.set_time_base(enc_tb);
    video.set_frame_rate(Some(ffmpeg::Rational(fps as i32, 1)));
    // Same value as the importer's `-g`. NOT u32::MAX: gop_size is a C int, so
    // u32::MAX truncates to -1 and x264 silently falls back to keyint=250,
    // inserting mid-clip keyframes that snap moshes back.
    video.set_gop(99_999_999);
    video.set_max_b_frames(0);
    if global_header {
        video.set_flags(ffmpeg::codec::Flags::GLOBAL_HEADER);
    }

    let mut opts = ffmpeg::Dictionary::new();
    // MUST match the importer's x264 config (preset, refs, no B-frames).
    // Mosh drops keyframes, so baked P-frames get parsed under the *source*
    // clip's SPS/PPS — structurally different headers make the decoder refuse
    // every frame and the span renders as one held frame.
    opts.set("preset", "veryfast");
    opts.set("refs", "1");
    opts.set("crf", "18");
    // Disable scene-cut I-frame insertion — baked clips must keep the
    // 1-keyframe long-GOP structure so mosh operations work on them.
    opts.set("sc_threshold", "0");

    let mut encoder = video.open_as_with(codec, opts)?;

    let mut ost = octx.add_stream(codec)?;
    ost.set_parameters(&encoder);
    ost.set_time_base(enc_tb);
    let stream_tb = ost.time_base();
    let ost_idx = ost.index();

    octx.write_header()?;

    let mut packet = ffmpeg::codec::packet::Packet::empty();
    let total = frames.len().max(1) as f32;

    for (i, yuv) in frames.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) { return Err(BakeError::Cancelled); }
        let mut frame = ffmpeg::util::frame::video::Video::new(
            ffmpeg::util::format::Pixel::YUV420P,
            width,
            height,
        );
        fill_video_frame(&mut frame, yuv);
        frame.set_pts(Some(i as i64));
        encoder.send_frame(&frame)?;
        while encoder.receive_packet(&mut packet).is_ok() {
            packet.set_stream(ost_idx);
            packet.rescale_ts(enc_tb, stream_tb);
            packet.write_interleaved(&mut octx)?;
        }
        progress(0.70 + 0.25 * ((i as f32 + 1.0) / total));
    }

    encoder.send_eof()?;
    while encoder.receive_packet(&mut packet).is_ok() {
        packet.set_stream(ost_idx);
        packet.rescale_ts(enc_tb, stream_tb);
        packet.write_interleaved(&mut octx)?;
    }

    octx.write_trailer()?;

    // Drop the output context so the file is flushed, then read it back.
    drop(octx);

    progress(0.95);
    if cancel.load(Ordering::Relaxed) { return Err(BakeError::Cancelled); }

    let mut ictx = ffmpeg::format::input(&path)?;
    let stream = ictx
        .streams()
        .best(ffmpeg::media::Type::Video)
        .ok_or_else(|| BakeError::Encode("no video stream in encoded output".into()))?;
    let idx = stream.index();
    let codec_parameters = stream.parameters();
    let time_base = stream.time_base();

    let mut packets: Vec<OwnedPacket> = Vec::new();
    for (s, p) in ictx.packets() {
        if s.index() != idx {
            continue;
        }
        let data: std::sync::Arc<[u8]> = p.data().unwrap_or(&[]).into();
        let is_key = p.flags().contains(ffmpeg::codec::packet::Flags::KEY);
        packets.push(OwnedPacket {
            data,
            pts: p.pts().unwrap_or(0),
            dts: p.dts().unwrap_or(0),
            duration: p.duration(),
            is_key,
        });
    }

    Ok(PacketClip {
        id: 0,
        name: "baked".into(),
        packets,
        width,
        height,
        codec_parameters,
        time_base,
    })
}

pub(crate) fn fill_video_frame(frame: &mut ffmpeg::util::frame::video::Video, yuv: &Yuv420) {
    let w = yuv.width as usize;
    let h = yuv.height as usize;
    let cw = yuv.chroma_width() as usize;
    let ch = yuv.chroma_height() as usize;

    let y_stride = frame.stride(0);
    let u_stride = frame.stride(1);
    let v_stride = frame.stride(2);
    copy_plane_into(frame.data_mut(0), y_stride, &yuv.y, w, h);
    copy_plane_into(frame.data_mut(1), u_stride, &yuv.u, cw, ch);
    copy_plane_into(frame.data_mut(2), v_stride, &yuv.v, cw, ch);
}

fn copy_plane_into(dst: &mut [u8], stride: usize, src: &[u8], width: usize, height: usize) {
    for row in 0..height {
        let dst_start = row * stride;
        let src_start = row * width;
        dst[dst_start..dst_start + width].copy_from_slice(&src[src_start..src_start + width]);
    }
}

// ------------------------------------------------------------------
// Tests
// ------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn solid_yuv(width: u32, height: u32, fill: u8) -> Yuv420 {
        let luma = (width * height) as usize;
        let chroma = ((width / 2) * (height / 2)) as usize;
        Yuv420 {
            width,
            height,
            y: vec![fill; luma],
            u: vec![128; chroma],
            v: vec![128; chroma],
        }
    }

    fn ramp_yuv(width: u32, height: u32) -> Yuv420 {
        let mut yuv = solid_yuv(width, height, 0);
        for (i, b) in yuv.y.iter_mut().enumerate() {
            *b = (i % 256) as u8;
        }
        yuv
    }

    #[test]
    fn reverse_scanlines_is_row_reversal() {
        let mut yuv = ramp_yuv(4, 2);
        // Row 0 before: [0, 1, 2, 3]
        bend_yuv(&mut yuv, BendMode::ReverseScanlines);
        assert_eq!(&yuv.y[..4], &[3, 2, 1, 0]);
        assert_eq!(&yuv.y[4..8], &[7, 6, 5, 4]);
    }

    #[test]
    fn bitcrush_masks_all_planes_uniformly() {
        let mut yuv = solid_yuv(4, 4, 0b1010_1010);
        // Simulate non-128 chroma to prove Bitcrush doesn't force it back.
        yuv.u.fill(0b1111_0000);
        yuv.v.fill(0b0011_0011);
        bend_yuv(&mut yuv, BendMode::Bitcrush { bits: 2 });
        // bits=2 → shift=6, mask=0b1100_0000
        assert!(yuv.y.iter().all(|&b| b == 0b1000_0000));
        assert!(yuv.u.iter().all(|&b| b == 0b1100_0000));
        assert!(yuv.v.iter().all(|&b| b == 0b0000_0000));
    }

    #[test]
    fn byteswap_reverses_full_stride() {
        let mut yuv = ramp_yuv(8, 1);
        bend_yuv(&mut yuv, BendMode::ByteSwap { stride: 4 });
        assert_eq!(yuv.y, vec![3, 2, 1, 0, 7, 6, 5, 4]);
    }

    #[test]
    fn xor_toggles_every_byte_on_all_planes() {
        let mut yuv = solid_yuv(4, 4, 0x0F);
        bend_yuv(&mut yuv, BendMode::Xor { mask: 0xFF });
        assert!(yuv.y.iter().all(|&b| b == 0xF0));
        assert!(yuv.u.iter().all(|&b| b == !128));
        assert!(yuv.v.iter().all(|&b| b == !128));
    }

    #[test]
    fn noise_is_deterministic_per_seed() {
        let mut a = solid_yuv(8, 8, 128);
        let mut b = solid_yuv(8, 8, 128);
        let mut c = solid_yuv(8, 8, 128);
        bend_yuv(&mut a, BendMode::Noise { amount: 16, seed: 42 });
        bend_yuv(&mut b, BendMode::Noise { amount: 16, seed: 42 });
        bend_yuv(&mut c, BendMode::Noise { amount: 16, seed: 43 });
        assert_eq!(a.y, b.y, "same seed → same output");
        assert_ne!(a.y, c.y, "different seed → different output");
    }

    #[test]
    fn pixel_sort_sorts_luma_runs_in_band() {
        // Grey pixels (chroma 128) survive the RGB round-trip exactly, so the
        // Y plane must come back sorted.
        let mut yuv = solid_yuv(6, 2, 0);
        yuv.y[..6].copy_from_slice(&[200, 50, 180, 90, 120, 30]);
        yuv.y[6..].copy_from_slice(&[200, 50, 180, 90, 120, 30]);
        bend_yuv(&mut yuv, BendMode::PixelSort {
            dir: SortDir::Horizontal,
            key: SortKey::Luma,
            lo: 0,
            hi: 255,
            reverse: false,
        });
        assert_eq!(&yuv.y[..6], &[30, 50, 90, 120, 180, 200]);
    }

    #[test]
    fn pixel_sort_leaves_pixels_outside_band_untouched() {
        let mut yuv = solid_yuv(6, 2, 0);
        yuv.y[..6].copy_from_slice(&[250, 50, 40, 30, 20, 250]);
        // Band 0..=100: only the middle run [50,40,30,20] sorts; 250s stay put.
        bend_yuv(&mut yuv, BendMode::PixelSort {
            dir: SortDir::Horizontal,
            key: SortKey::Luma,
            lo: 0,
            hi: 100,
            reverse: false,
        });
        assert_eq!(&yuv.y[..6], &[250, 20, 30, 40, 50, 250]);
    }

    #[test]
    fn pixel_sort_vertical_reverse_descends() {
        let mut yuv = solid_yuv(2, 4, 0);
        for (i, v) in [10u8, 10, 200, 200, 40, 40, 90, 90].iter().enumerate() {
            yuv.y[i] = *v;
        }
        bend_yuv(&mut yuv, BendMode::PixelSort {
            dir: SortDir::Vertical,
            key: SortKey::Luma,
            lo: 0,
            hi: 255,
            reverse: true,
        });
        // Column 0 was [10, 200, 40, 90] → descending.
        assert_eq!([yuv.y[0], yuv.y[2], yuv.y[4], yuv.y[6]], [200, 90, 40, 10]);
    }

    #[test]
    fn ringing_ghosts_after_edge_but_leaves_flat_areas_alone() {
        let mut flat = solid_yuv(32, 2, 128);
        bend_yuv(&mut flat, BendMode::Ringing { amount: 1.0, period: 4 });
        assert!(flat.y.iter().all(|&b| b == 128), "no edges → no change");

        // Step edge at x=8: 0 → 200.
        let mut yuv = solid_yuv(32, 2, 0);
        for row in 0..2 {
            for x in 8..32 {
                yuv.y[row * 32 + x] = 200;
            }
        }
        bend_yuv(&mut yuv, BendMode::Ringing { amount: 0.5, period: 4 });
        // First tap: edge echoed 4px after the step.
        assert!(yuv.y[12] > 200, "positive ghost expected at x=12, got {}", yuv.y[12]);
        assert!(yuv.y[16] < 200, "negative ghost expected at x=16, got {}", yuv.y[16]);
    }

    #[test]
    fn chroma_shift_wraps_planes_in_opposite_directions() {
        let mut yuv = solid_yuv(8, 8, 128);
        // chroma planes are 4×4
        yuv.u[0] = 10; // (0,0)
        yuv.v[0] = 20;
        bend_yuv(&mut yuv, BendMode::ChromaShift { dx: 1, dy: 0, wrap: true });
        assert_eq!(yuv.u[1], 10, "U shifts +x");
        assert_eq!(yuv.v[3], 20, "V shifts -x, wrapping to the last column");
        assert_eq!(yuv.y, solid_yuv(8, 8, 128).y, "luma untouched");
    }

    #[test]
    fn chroma_shift_clamp_extends_edges_instead_of_wrapping() {
        let mut yuv = solid_yuv(8, 8, 128);
        yuv.u[0] = 10; // (0,0) in the 4×4 chroma plane
        bend_yuv(&mut yuv, BendMode::ChromaShift { dx: 1, dy: 0, wrap: false });
        assert_eq!(yuv.u[1], 10, "U shifts +x");
        assert_eq!(yuv.u[0], 10, "left edge clamps, repeating the edge value");
        assert_eq!(yuv.u[3], 128, "nothing wraps in from the far edge");
    }

    #[test]
    fn noise_reseeds_per_frame_so_grain_animates() {
        let mode = BendMode::Noise { amount: 16, seed: 7 };
        let mut a = solid_yuv(8, 8, 128);
        let mut b = solid_yuv(8, 8, 128);
        bend_yuv(&mut a, mode.for_frame(0));
        bend_yuv(&mut b, mode.for_frame(1));
        assert_ne!(a.y, b.y, "consecutive frames must get different noise");
        // Non-noise modes are frame-invariant.
        assert_eq!(
            BendMode::Bitcrush { bits: 3 }.for_frame(9),
            BendMode::Bitcrush { bits: 3 },
        );
    }

    #[test]
    fn compress_region_errors_when_origin_out_of_bounds() {
        let mut yuv = solid_yuv(16, 16, 100);
        let out = compress_region(
            &mut yuv,
            &CompressRegion { x: 20, y: 0, w: 4, h: 4, quality: 10 },
        );
        assert!(matches!(out, Err(BakeError::InvalidRegion { .. })));
    }

    #[test]
    fn compress_region_errors_on_zero_size() {
        let mut yuv = solid_yuv(16, 16, 100);
        let out = compress_region(
            &mut yuv,
            &CompressRegion { x: 0, y: 0, w: 0, h: 4, quality: 10 },
        );
        assert!(matches!(out, Err(BakeError::InvalidRegion { .. })));
    }

    #[test]
    fn baked_encode_keeps_single_leading_keyframe_across_scene_cuts() {
        ffmpeg::init().unwrap();
        // Hard scene change every 10 frames — without sc_threshold=0 x264
        // inserts an I-frame at each cut, breaking the long-GOP mosh model.
        // 260 frames also crosses x264's default keyint of 250, which kicks in
        // silently if the huge GOP value doesn't survive the C int conversion.
        let mut frames = Vec::new();
        for i in 0..260usize {
            let mut f = solid_yuv(128, 128, if (i / 10) % 2 == 0 { 20 } else { 235 });
            if (i / 10) % 2 == 1 {
                for (j, b) in f.y.iter_mut().enumerate() {
                    *b = ((j * 7 + i) % 256) as u8;
                }
            }
            frames.push(f);
        }
        let cancel = AtomicBool::new(false);
        let clip = encode_yuv_to_packet_clip(&frames, 30, &mut |_| {}, &cancel).unwrap();
        assert_eq!(clip.packets.len(), 260);
        assert_eq!(clip.keyframe_indices(), vec![0]);
    }

    /// Mean absolute luma difference between two frames.
    fn mean_abs_diff(a: &[u8], b: &[u8]) -> f64 {
        a.iter()
            .zip(b)
            .map(|(x, y)| (*x as f64 - *y as f64).abs())
            .sum::<f64>()
            / a.len() as f64
    }

    #[test]
    fn bake_pixel_sort_and_ringing_preserve_motion() {
        ffmpeg::init().unwrap();
        // 30 frames of a bright block sliding 4 px per frame on a dark field.
        let mut frames = Vec::new();
        for i in 0..30usize {
            let mut f = solid_yuv(128, 128, 30);
            for y in 20..36 {
                for x in (i * 4)..(i * 4 + 16) {
                    f.y[y * 128 + x] = 220;
                }
            }
            frames.push(f);
        }
        let cancel = AtomicBool::new(false);
        let clip = encode_yuv_to_packet_clip(&frames, 30, &mut |_| {}, &cancel).unwrap();

        for effect in [
            Effect::Bend(BendMode::PixelSort {
                dir: SortDir::Horizontal,
                key: SortKey::Luma,
                lo: 100,
                hi: 255,
                reverse: false,
            }),
            Effect::Bend(BendMode::Ringing { amount: 0.8, period: 4 }),
        ] {
            // Mid-clip range so the keyframe-seek decode path is exercised too.
            let baked =
                bake_segment(&clip, 5, 20, effect.clone(), 30, &mut |_| {}, &cancel).unwrap();
            assert_eq!(baked.packets.len(), 20, "{effect:?}: wrong frame count");

            let refs: Vec<&OwnedPacket> = baked.packets.iter().collect();
            let mut dec = PacketDecoder::new(&baked.codec_parameters).unwrap();
            let out = dec.decode_all(&refs).unwrap();
            assert_eq!(out.len(), 20, "{effect:?}: baked clip lost frames");

            let moved = out
                .iter()
                .skip(10)
                .any(|f| mean_abs_diff(&f.y, &out[0].y) > 2.0);
            assert!(moved, "{effect:?}: baked output is frozen on its first frame");
        }
    }

    #[test]
    fn baked_overlay_renders_motion_through_wysiwyg() {
        ffmpeg::init().unwrap();
        // 60-frame moving source; frames 20..40 baked with pixel sort and
        // overlaid on V1 exactly like finish_bake does. The WYSIWYG render of
        // that two-track sequence must keep motion inside the baked span.
        let mut frames = Vec::new();
        for i in 0..60usize {
            let mut f = solid_yuv(128, 128, 30);
            let x0 = (i * 2).min(110);
            for y in 20..36 {
                for x in x0..x0 + 16 {
                    f.y[y * 128 + x] = 220;
                }
            }
            frames.push(f);
        }
        let cancel = AtomicBool::new(false);
        let source = encode_yuv_to_packet_clip(&frames, 30, &mut |_| {}, &cancel).unwrap();
        let effect = Effect::Bend(BendMode::PixelSort {
            dir: SortDir::Horizontal,
            key: SortKey::Luma,
            lo: 100,
            hi: 255,
            reverse: false,
        });
        let baked = bake_segment(&source, 20, 20, effect, 30, &mut |_| {}, &cancel).unwrap();

        // Variant 1: baked clip keeps its keyframe (plain overlay).
        // Variant 2: leading keyframe dropped (cross-clip mosh) — the baked
        // P-frames bleed from the source's decoder state.
        for drop_key in [false, true] {
            let mut seq: Vec<(OwnedPacket, usize)> = Vec::new();
            for f in 0..60usize {
                let (pkt, owner) = if (20..40).contains(&f) {
                    let skip = if drop_key { 1 } else { 0 };
                    match baked.packets.get(f - 20 + skip) {
                        Some(p) => (p, 1usize),
                        None => (&source.packets[f], 0usize),
                    }
                } else {
                    (&source.packets[f], 0usize)
                };
                let i = seq.len() as i64;
                seq.push((
                    OwnedPacket {
                        data: pkt.data.clone(),
                        pts: i,
                        dts: i,
                        duration: 1,
                        is_key: pkt.is_key,
                    },
                    owner,
                ));
            }

            let params = [source.codec_parameters.clone(), baked.codec_parameters.clone()];
            let tmp = tempfile::tempdir().unwrap();
            let out_path = tmp.path().join("wys.mp4");
            let emitted = crate::render::wysiwyg::bake_sequence_to_mp4(
                &seq, &params, 128, 128, 30, &out_path,
            )
            .unwrap();
            assert_eq!(emitted, 60);

            let clip = crate::importer::read_clip_from_mp4(&out_path, "check", 0).unwrap();
            let refs: Vec<&OwnedPacket> = clip.packets.iter().collect();
            let mut dec = PacketDecoder::new(&clip.codec_parameters).unwrap();
            let out = dec.decode_all(&refs).unwrap();
            assert_eq!(out.len(), 60);

            let moved = (22..38).any(|i| mean_abs_diff(&out[i].y, &out[21].y) > 2.0);
            assert!(
                moved,
                "baked overlay (drop_key={drop_key}) is frozen in the rendered output"
            );
        }
    }

    /// Mirror of the preview worker's `playing` fast path: one decoder, packets
    /// fed strictly in sequence order, newest surfaced frame shown per step.
    /// The user's real timelines have a single keyframe at position 0 (every
    /// later span is mid-GOP or mosh-dropped), so playback must still show
    /// motion inside a baked pixel-sort overlay.
    #[test]
    fn sequential_feed_playback_shows_motion_in_baked_overlay() {
        ffmpeg::init().unwrap();
        let mut frames = Vec::new();
        for i in 0..60usize {
            let mut f = solid_yuv(128, 128, 30);
            let x0 = (i * 2).min(110);
            for y in 20..36 {
                for x in x0..x0 + 16 {
                    f.y[y * 128 + x] = 220;
                }
            }
            frames.push(f);
        }
        let cancel = AtomicBool::new(false);
        let source = encode_yuv_to_packet_clip(&frames, 30, &mut |_| {}, &cancel).unwrap();
        let effect = Effect::Bend(BendMode::PixelSort {
            dir: SortDir::Horizontal,
            key: SortKey::Luma,
            lo: 64,
            hi: 192,
            reverse: false,
        });
        let baked = bake_segment(&source, 20, 20, effect, 30, &mut |_| {}, &cancel).unwrap();

        // Timeline shape: source 0..20, baked overlay (keyframe dropped, mosh)
        // 20..39, source resumes mid-GOP 39..60.
        let mut seq: Vec<OwnedPacket> = Vec::new();
        for f in 0..60usize {
            let pkt = if (20..39).contains(&f) {
                &baked.packets[f - 20 + 1]
            } else {
                &source.packets[f]
            };
            let i = seq.len() as i64;
            seq.push(OwnedPacket {
                data: pkt.data.clone(),
                pts: i,
                dts: i,
                duration: 1,
                is_key: pkt.is_key,
            });
        }

        let mut dec = PacketDecoder::new(&source.codec_parameters).unwrap();
        let mut shown: Vec<std::sync::Arc<Yuv420>> = Vec::new();
        let mut last: Option<std::sync::Arc<Yuv420>> = None;
        for pkt in &seq {
            if let Some(y) = dec.feed(pkt) {
                last = Some(y);
            }
            if let Some(y) = &last {
                shown.push(y.clone());
            }
        }
        assert!(shown.len() >= 50, "playback produced too few frames");

        // Compare what's on screen at the start vs the end of the baked span.
        let a = &shown[shown.len().saturating_sub(38)];
        let moved = shown[shown.len() - 30..]
            .iter()
            .any(|f| mean_abs_diff(&f.y, &a.y) > 2.0);
        assert!(moved, "sequential playback is frozen across the baked overlay");
    }

    /// When a baked V1 overlay ends, the V2 source resumes mid-GOP and its
    /// P-frames must decode against the *baked* final state (bleed/melt) — not
    /// snap instantly back to the clean source. A snap would mean the render
    /// resynced the decoder where no keyframe exists.
    #[test]
    fn overlay_end_bleeds_instead_of_snapping_clean() {
        ffmpeg::init().unwrap();
        let mut frames = Vec::new();
        for i in 0..60usize {
            let mut f = solid_yuv(128, 128, 30);
            let x0 = (i * 2).min(110);
            for y in 20..36 {
                for x in x0..x0 + 16 {
                    f.y[y * 128 + x] = 220;
                }
            }
            frames.push(f);
        }
        let cancel = AtomicBool::new(false);
        let source = encode_yuv_to_packet_clip(&frames, 30, &mut |_| {}, &cancel).unwrap();
        // Strong, frame-filling effect so the bleed is unmistakable.
        let effect = Effect::Bend(BendMode::Xor { mask: 0xFF });
        let baked = bake_segment(&source, 20, 20, effect, 30, &mut |_| {}, &cancel).unwrap();

        // V1 overlay (keyframe intact) at 20..40, V2 resumes mid-GOP at 40.
        let mut seq: Vec<(OwnedPacket, usize)> = Vec::new();
        for f in 0..60usize {
            let (pkt, owner) = if (20..40).contains(&f) {
                (&baked.packets[f - 20], 1usize)
            } else {
                (&source.packets[f], 0usize)
            };
            let i = seq.len() as i64;
            seq.push((
                OwnedPacket {
                    data: pkt.data.clone(),
                    pts: i,
                    dts: i,
                    duration: 1,
                    is_key: pkt.is_key,
                },
                owner,
            ));
        }
        let params = [source.codec_parameters.clone(), baked.codec_parameters.clone()];
        let tmp = tempfile::tempdir().unwrap();
        let out_path = tmp.path().join("wys.mp4");
        crate::render::wysiwyg::bake_sequence_to_mp4(&seq, &params, 128, 128, 30, &out_path)
            .unwrap();

        let clip = crate::importer::read_clip_from_mp4(&out_path, "check", 0).unwrap();
        let refs: Vec<&OwnedPacket> = clip.packets.iter().collect();
        let mut dec = PacketDecoder::new(&clip.codec_parameters).unwrap();
        let rendered = dec.decode_all(&refs).unwrap();

        // Clean reference: decode the source alone.
        let src_refs: Vec<&OwnedPacket> = source.packets.iter().collect();
        let mut dec2 = PacketDecoder::new(&source.codec_parameters).unwrap();
        let clean = dec2.decode_all(&src_refs).unwrap();

        // Frame 40 (first after the overlay) must still carry the baked state:
        // far from the clean source frame. By frame 59 it may have melted back.
        let resume_diff = mean_abs_diff(&rendered[40].y, &clean[40].y);
        assert!(
            resume_diff > 20.0,
            "V2 resume snapped clean (diff {resume_diff:.1}) — decoder was \
             resynced at the overlay end where no keyframe exists"
        );
    }

    #[test]
    fn compress_region_clamps_overlap_and_succeeds() {
        let mut yuv = solid_yuv(16, 16, 100);
        // Request runs past the right edge — must be clamped, not rejected.
        let out = compress_region(
            &mut yuv,
            &CompressRegion { x: 12, y: 12, w: 99, h: 99, quality: 50 },
        );
        assert!(out.is_ok());
    }
}
