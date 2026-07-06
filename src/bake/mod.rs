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

#[derive(Debug, Clone, Copy)]
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
            Effect::Bend(mode) => bend_yuv(yuv, *mode),
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
    video.set_gop(u32::MAX);
    video.set_max_b_frames(0);
    if global_header {
        video.set_flags(ffmpeg::codec::Flags::GLOBAL_HEADER);
    }

    let mut opts = ffmpeg::Dictionary::new();
    opts.set("preset", "fast");
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
        let data = p.data().unwrap_or(&[]).to_vec();
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
        let mut frames = Vec::new();
        for i in 0..40usize {
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
        assert_eq!(clip.packets.len(), 40);
        assert_eq!(clip.keyframe_indices(), vec![0]);
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
