use std::path::Path;
use std::process::{Command, Stdio};

use ffmpeg_next as ffmpeg;
use thiserror::Error;

use crate::codec::ir::Yuv420;
use crate::packet::{OwnedPacket, PacketClip};

#[derive(Debug, Error)]
pub enum ImportError {
    #[error("ffmpeg error: {0}")]
    Ffmpeg(#[from] ffmpeg::Error),
    #[error("no video stream found in file")]
    NoVideoStream,
    #[error("ffmpeg transcoding failed: {0}")]
    TranscodeFailed(String),
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
}

/// Fixed project resolution. All clips are normalized to this size on import.
const PROJECT_WIDTH: u32 = 1920;
const PROJECT_HEIGHT: u32 = 1080;

/// Transcode the source video to a long-GOP H.264 file, read its packets,
/// and return a `PacketClip` plus the first decoded YUV frame.
pub fn import_video(path: &Path, name: impl Into<String>) -> Result<(PacketClip, Yuv420), ImportError> {
    ffmpeg::init()?;

    let temp_dir = tempfile::tempdir()?;
    let temp_path = temp_dir.path().join("transcoded.mp4");

    // 1. Transcode with ffmpeg CLI to guarantee one I-frame + all P-frames.
    let output = Command::new(crate::bundled_ffmpeg())
        .args([
            "-y",
            "-i", path.to_str().unwrap_or(""),
            "-vf", &format!("scale={}:{}", PROJECT_WIDTH, PROJECT_HEIGHT),
            "-vcodec", "libx264",
            "-g", "99999999",
            // Without this x264 still inserts I-frames at scene changes,
            // breaking the 1-keyframe model and snapping moshes back mid-bleed.
            "-sc_threshold", "0",
            "-bf", "0",
            "-pix_fmt", "yuv420p",
            "-movflags", "faststart",
            // veryfast ≈ halves import time vs fast; CRF still governs quality.
            // MUST stay in lockstep with bake::encode_yuv_to_packet_clip —
            // mismatched SPS/PPS structure (preset, refs) makes moshed clips
            // undecodable against each other and renders freeze on one frame.
            "-preset", "veryfast",
            "-refs", "1",
            "-crf", "18",
            temp_path.to_str().unwrap_or(""),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| ImportError::TranscodeFailed(format!("ffmpeg launch failed: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let msg = if stderr.trim().is_empty() {
            format!("exit code {}", output.status)
        } else {
            stderr.trim().to_string()
        };
        return Err(ImportError::TranscodeFailed(msg));
    }

    // 2. Read packets from the transcoded file.
    let mut ictx = ffmpeg::format::input(&temp_path)?;
    let stream = ictx
        .streams()
        .best(ffmpeg::media::Type::Video)
        .ok_or(ImportError::NoVideoStream)?;
    let stream_idx = stream.index();
    let time_base = stream.time_base();
    let codec_parameters = stream.parameters();

    let mut packets: Vec<OwnedPacket> = Vec::new();
    for (s, packet) in ictx.packets() {
        if s.index() != stream_idx {
            continue;
        }
        let data: std::sync::Arc<[u8]> = packet.data().unwrap_or(&[]).into();
        let is_key = packet.flags().contains(ffmpeg::codec::packet::Flags::KEY);
        packets.push(OwnedPacket {
            data,
            pts: packet.pts().unwrap_or(0),
            dts: packet.dts().unwrap_or(0),
            duration: packet.duration(),
            is_key,
        });
    }

    // 3. Decode the first packet to grab dimensions and a preview frame.
    let mut decoder = ffmpeg::codec::context::Context::from_parameters(codec_parameters.clone())?
        .decoder()
        .video()?;
    let first_packet = ffmpeg::codec::packet::Packet::copy(&packets[0].data);
    decoder.send_packet(&first_packet)?;
    let mut frame = ffmpeg::util::frame::video::Video::empty();
    decoder.receive_frame(&mut frame)?;

    let yuv = copy_yuv_from_frame(&frame);

    let clip = PacketClip {
        id: 0, // caller will assign
        name: name.into(),
        packets,
        width: PROJECT_WIDTH,
        height: PROJECT_HEIGHT,
        codec_parameters,
        time_base,
    };

    Ok((clip, yuv))
}

/// Read an already-normalized long-GOP MP4 back into a `PacketClip` without
/// re-transcoding. Used when loading a saved project bundle, whose `media/*.mp4`
/// files were written by [`crate::render::muxer::export_packets`] and are
/// therefore already in the project's I+P packet layout.
pub fn read_clip_from_mp4(
    path: &Path,
    name: impl Into<String>,
    id: u64,
) -> Result<PacketClip, ImportError> {
    ffmpeg::init()?;

    let mut ictx = ffmpeg::format::input(&path)?;
    let stream = ictx
        .streams()
        .best(ffmpeg::media::Type::Video)
        .ok_or(ImportError::NoVideoStream)?;
    let stream_idx = stream.index();
    let time_base = stream.time_base();
    let codec_parameters = stream.parameters();
    // All project media is normalized to the project resolution on import.
    let width = PROJECT_WIDTH;
    let height = PROJECT_HEIGHT;

    let mut packets: Vec<OwnedPacket> = Vec::new();
    for (s, packet) in ictx.packets() {
        if s.index() != stream_idx {
            continue;
        }
        let data: std::sync::Arc<[u8]> = packet.data().unwrap_or(&[]).into();
        let is_key = packet.flags().contains(ffmpeg::codec::packet::Flags::KEY);
        packets.push(OwnedPacket {
            data,
            pts: packet.pts().unwrap_or(0),
            dts: packet.dts().unwrap_or(0),
            duration: packet.duration(),
            is_key,
        });
    }

    Ok(PacketClip {
        id,
        name: name.into(),
        packets,
        width,
        height,
        codec_parameters,
        time_base,
    })
}

fn copy_yuv_from_frame(frame: &ffmpeg::util::frame::video::Video) -> Yuv420 {
    let width = frame.width();
    let height = frame.height();
    let cw = (width / 2) as usize;
    let ch = (height / 2) as usize;

    let y = copy_plane(frame.data(0), frame.stride(0), width as usize, height as usize);
    let u = copy_plane(frame.data(1), frame.stride(1), cw, ch);
    let v = copy_plane(frame.data(2), frame.stride(2), cw, ch);

    Yuv420 { width, height, y, u, v }
}

fn copy_plane(src: &[u8], stride: usize, width: usize, height: usize) -> Vec<u8> {
    let mut dst = Vec::with_capacity(width * height);
    for row in 0..height {
        dst.extend_from_slice(&src[row * stride..row * stride + width]);
    }
    dst
}
