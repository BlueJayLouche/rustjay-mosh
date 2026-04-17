use std::sync::Arc;

use ffmpeg_next as ffmpeg;
use thiserror::Error;

use crate::codec::ir::Yuv420;
use crate::packet::OwnedPacket;

#[derive(Debug, Error)]
pub enum DecodeError {
    #[error("ffmpeg error: {0}")]
    Ffmpeg(#[from] ffmpeg::Error),
    #[error("no frame decoded")]
    NoFrame,
}

/// Wraps an ffmpeg video decoder and can decode a slice of packets up to a
/// specific index. It handles flushing when jumping back to a keyframe.
pub struct PacketDecoder {
    decoder: ffmpeg::decoder::Video,
}

impl PacketDecoder {
    pub fn new(parameters: &ffmpeg::codec::Parameters) -> Result<Self, DecodeError> {
        let mut decoder = ffmpeg::codec::context::Context::from_parameters(parameters.clone())?
            .decoder()
            .video()?;
        decoder.set_threading(ffmpeg::codec::threading::Config::count(4));
        Ok(Self { decoder })
    }

    /// Decode packets up to `target_idx` (inclusive) and return the last
    /// decoded YUV420 frame. To handle scrubbing, the decoder is flushed and
    /// decoding restarts from the nearest preceding keyframe.
    pub fn decode_up_to(
        &mut self,
        packets: &[&OwnedPacket],
        target_idx: usize,
    ) -> Result<Arc<Yuv420>, DecodeError> {
        if packets.is_empty() {
            return Err(DecodeError::NoFrame);
        }
        let target_idx = target_idx.min(packets.len() - 1);

        // Find nearest keyframe at or before target.
        let keyframe_start = packets[..=target_idx]
            .iter()
            .enumerate()
            .rev()
            .find(|(_, p)| p.is_key)
            .map(|(i, _)| i)
            .unwrap_or(0);

        // Flush decoder state so we can restart from the keyframe safely.
        self.decoder.flush();

        let mut frame = ffmpeg::util::frame::video::Video::empty();
        let mut last_yuv: Option<Yuv420> = None;

        for idx in keyframe_start..=target_idx {
            let pkt = packets[idx];
            let mut packet = ffmpeg::codec::packet::Packet::copy(&pkt.data);
            packet.set_pts(Some(pkt.pts));
            packet.set_dts(Some(pkt.dts));
            packet.set_duration(pkt.duration);
            if pkt.is_key {
                packet.set_flags(ffmpeg::codec::packet::Flags::KEY);
            }
            self.decoder.send_packet(&packet)?;
            while self.decoder.receive_frame(&mut frame).is_ok() {
                last_yuv = Some(copy_yuv_from_frame(&frame));
            }
        }

        // Drain any buffered frames.
        self.decoder.send_eof()?;
        while self.decoder.receive_frame(&mut frame).is_ok() {
            last_yuv = Some(copy_yuv_from_frame(&frame));
        }

        last_yuv.map(Arc::new).ok_or(DecodeError::NoFrame)
    }
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
