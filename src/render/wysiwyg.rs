//! WYSIWYG render: decode the moshed packet sequence in-app (tolerantly, the
//! same way the preview does) and re-encode the decoded pixels to a clean,
//! legal H.264 stream.
//!
//! Rationale: a direct packet remux hands players a bitstream whose P-frames
//! reference pictures that were never sent (that's the mosh). ffmpeg's decoder
//! silently drops 3–7 frames at every moshed transition, and stricter players
//! (QuickTime / VideoToolbox) can stop decoding entirely — freezing on the
//! first frame of an overlaid clip. Baking the glitch into decoded pixels
//! fixes both: output length always equals the timeline length and the file
//! plays everywhere.

use std::path::Path;

use ffmpeg_next as ffmpeg;
use thiserror::Error;

use crate::codec::ir::Yuv420;
use crate::packet::OwnedPacket;

#[derive(Debug, Error)]
pub enum WysiwygError {
    #[error("ffmpeg error: {0}")]
    Ffmpeg(#[from] ffmpeg::Error),
    #[error("encode error: {0}")]
    Encode(String),
    #[error("no packets to render")]
    Empty,
}

fn new_decoder(params: &ffmpeg::codec::Parameters) -> Result<ffmpeg::decoder::Video, WysiwygError> {
    let mut d = ffmpeg::codec::context::Context::from_parameters(params.clone())?
        .decoder()
        .video()?;
    d.set_threading(ffmpeg::codec::threading::Config::count(4));
    Ok(d)
}

/// Decode `packets` (pts/dts pre-stamped to their global sequence index) and
/// encode every timeline frame to `out` as visually-lossless H.264.
///
/// Each packet is tagged with the pool-clip index it came from; whenever a
/// keyframe owned by a different clip arrives, the decoder is rebuilt with
/// that clip's codec parameters — mirroring the preview worker, so clips with
/// different SPS/PPS decode correctly. Frames the decoder refuses to produce
/// (broken references right after a moshed cut) are covered by holding the
/// last good frame, keeping output frame count == packet count so A/V sync
/// is preserved.
pub fn bake_sequence_to_mp4(
    packets: &[(OwnedPacket, usize)],
    clip_params: &[ffmpeg::codec::Parameters],
    width: u32,
    height: u32,
    fps: u32,
    out: &Path,
) -> Result<usize, WysiwygError> {
    if packets.is_empty() {
        return Err(WysiwygError::Empty);
    }

    // ── Encoder / muxer setup ────────────────────────────────────────────
    let codec = ffmpeg::encoder::find(ffmpeg::codec::Id::H264)
        .ok_or_else(|| WysiwygError::Encode("libx264 encoder not available".into()))?;

    let mut octx = ffmpeg::format::output(&out)?;
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
    // Closed 2 s GOP: keyframes make the output seekable and robust; the
    // glitch is already baked into the pixels so they cost nothing visually.
    video.set_gop(fps * 2);
    video.set_max_b_frames(0);
    if global_header {
        video.set_flags(ffmpeg::codec::Flags::GLOBAL_HEADER);
    }

    let mut opts = ffmpeg::Dictionary::new();
    opts.set("preset", "veryfast");
    opts.set("crf", "12");

    let mut encoder = video.open_as_with(codec, opts)?;

    let mut ost = octx.add_stream(codec)?;
    ost.set_parameters(&encoder);
    ost.set_time_base(enc_tb);
    let ost_idx = ost.index();

    octx.write_header()?;
    // MP4 may override the requested time_base with its own timescale; read
    // back the actual one after write_header and rescale into it.
    let stream_tb = octx
        .stream(ost_idx)
        .ok_or_else(|| WysiwygError::Encode("no output stream".into()))?
        .time_base();

    let mut enc_pkt = ffmpeg::codec::packet::Packet::empty();
    let mut emitted: i64 = 0;
    let mut last_yuv: Option<Yuv420> = None;

    let emit =
        |yuv: &Yuv420,
         encoder: &mut ffmpeg::encoder::Video,
         octx: &mut ffmpeg::format::context::Output,
         emitted: &mut i64|
         -> Result<(), WysiwygError> {
            let mut frame = ffmpeg::util::frame::video::Video::new(
                ffmpeg::util::format::Pixel::YUV420P,
                width,
                height,
            );
            crate::bake::fill_video_frame(&mut frame, yuv);
            frame.set_pts(Some(*emitted));
            *emitted += 1;
            encoder.send_frame(&frame)?;
            let mut pkt = ffmpeg::codec::packet::Packet::empty();
            while encoder.receive_packet(&mut pkt).is_ok() {
                pkt.set_stream(ost_idx);
                // Explicit 1-frame duration; without it the muxer's edit list
                // comes up short and players discard the final frame.
                pkt.set_duration(1);
                pkt.rescale_ts(enc_tb, stream_tb);
                pkt.write_interleaved(octx)?;
            }
            Ok(())
        };

    // ── Decode loop ──────────────────────────────────────────────────────
    let mut dec_owner = packets[0].1;
    let mut decoder = new_decoder(clip_params.get(dec_owner).unwrap_or(&clip_params[0]))?;
    let mut dec_frame = ffmpeg::util::frame::video::Video::empty();

    // Fill the gap up to `pts` with the last good frame (black if none yet),
    // then emit the frame itself.
    macro_rules! drain_decoder {
        ($dec:expr) => {
            while $dec.receive_frame(&mut dec_frame).is_ok() {
                let pts = dec_frame.pts().unwrap_or(emitted);
                let yuv = crate::preview::decoder::copy_yuv_from_frame(&dec_frame);
                while emitted < pts {
                    let hold = last_yuv.clone().unwrap_or_else(|| Yuv420::new(width, height));
                    emit(&hold, &mut encoder, &mut octx, &mut emitted)?;
                }
                if emitted == pts {
                    emit(&yuv, &mut encoder, &mut octx, &mut emitted)?;
                }
                last_yuv = Some(yuv);
            }
        };
    }

    for (pkt, owner) in packets {
        if pkt.is_key && *owner != dec_owner {
            // Flush the old decoder fully, then start fresh with the new
            // clip's codec parameters (different SPS/PPS).
            decoder.send_eof()?;
            drain_decoder!(decoder);
            dec_owner = *owner;
            decoder = new_decoder(clip_params.get(dec_owner).unwrap_or(&clip_params[0]))?;
        }
        let mut packet = ffmpeg::codec::packet::Packet::copy(&pkt.data);
        packet.set_pts(Some(pkt.pts));
        packet.set_dts(Some(pkt.dts));
        packet.set_duration(pkt.duration);
        if pkt.is_key {
            packet.set_flags(ffmpeg::codec::packet::Flags::KEY);
        }
        // A moshed stream is intentionally malformed; ignore per-packet
        // decode errors and keep feeding.
        let _ = decoder.send_packet(&packet);
        drain_decoder!(decoder);
    }
    let _ = decoder.send_eof();
    drain_decoder!(decoder);

    // Pad the tail so every timeline frame exists in the output.
    let total = packets.len() as i64;
    while emitted < total {
        let hold = last_yuv.clone().unwrap_or_else(|| Yuv420::new(width, height));
        emit(&hold, &mut encoder, &mut octx, &mut emitted)?;
    }

    encoder.send_eof()?;
    while encoder.receive_packet(&mut enc_pkt).is_ok() {
        enc_pkt.set_stream(ost_idx);
        enc_pkt.set_duration(1);
        enc_pkt.rescale_ts(enc_tb, stream_tb);
        enc_pkt.write_interleaved(&mut octx)?;
    }
    octx.write_trailer()?;

    Ok(emitted as usize)
}
