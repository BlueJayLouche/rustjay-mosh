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

    /// Reset decoder state (flush) so a fresh keyframe-rooted feed can begin.
    pub fn reset(&mut self) {
        self.decoder.flush();
    }

    /// Feed one packet and return the newest decoded frame, if one surfaced.
    /// Used for sequential playback: no EOF is sent, so the stream can keep
    /// going with further `feed` calls. With threaded decoding the returned
    /// frame may lag the fed packet by a few frames — invisible during
    /// playback; use [`decode_up_to`](Self::decode_up_to) for exact scrubbing.
    pub fn feed(&mut self, pkt: &OwnedPacket) -> Option<Arc<Yuv420>> {
        let mut packet = ffmpeg::codec::packet::Packet::copy(&pkt.data);
        packet.set_pts(Some(pkt.pts));
        packet.set_dts(Some(pkt.dts));
        packet.set_duration(pkt.duration);
        if pkt.is_key {
            packet.set_flags(ffmpeg::codec::packet::Flags::KEY);
        }
        if self.decoder.send_packet(&packet).is_err() {
            return None;
        }
        let mut frame = ffmpeg::util::frame::video::Video::empty();
        let mut last = None;
        while self.decoder.receive_frame(&mut frame).is_ok() {
            last = Some(Arc::new(copy_yuv_from_frame(&frame)));
        }
        last
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

    /// Decode `count` frames starting at `start`, seeking to the nearest
    /// preceding keyframe and discarding any pre-roll frames so only the
    /// requested range is returned.
    ///
    /// Assumes no B-frames (each packet produces one frame in presentation
    /// order) — which holds for clips produced by this project's importer.
    pub fn decode_range(
        &mut self,
        packets: &[&OwnedPacket],
        start: usize,
        count: usize,
    ) -> Result<Vec<Arc<Yuv420>>, DecodeError> {
        if packets.is_empty() || count == 0 {
            return Ok(vec![]);
        }
        let end = (start + count).min(packets.len());
        if start >= end {
            return Ok(vec![]);
        }

        let keyframe_start = packets[..=start]
            .iter()
            .enumerate()
            .rev()
            .find(|(_, p)| p.is_key)
            .map(|(i, _)| i)
            .unwrap_or(0);

        self.decoder.flush();

        let mut frame = ffmpeg::util::frame::video::Video::empty();
        let want = end - start;
        let mut frames: Vec<Arc<Yuv420>> = Vec::with_capacity(want);
        let mut next_out_idx = keyframe_start;

        for pkt in &packets[keyframe_start..end] {
            let mut packet = ffmpeg::codec::packet::Packet::copy(&pkt.data);
            packet.set_pts(Some(pkt.pts));
            packet.set_dts(Some(pkt.dts));
            packet.set_duration(pkt.duration);
            if pkt.is_key {
                packet.set_flags(ffmpeg::codec::packet::Flags::KEY);
            }
            self.decoder.send_packet(&packet)?;
            while self.decoder.receive_frame(&mut frame).is_ok() {
                if next_out_idx >= start && frames.len() < want {
                    frames.push(Arc::new(copy_yuv_from_frame(&frame)));
                }
                next_out_idx += 1;
            }
            if frames.len() >= want {
                break;
            }
        }

        if frames.len() < want {
            self.decoder.send_eof()?;
            while self.decoder.receive_frame(&mut frame).is_ok() {
                if next_out_idx >= start && frames.len() < want {
                    frames.push(Arc::new(copy_yuv_from_frame(&frame)));
                }
                next_out_idx += 1;
                if frames.len() >= want {
                    break;
                }
            }
        }

        Ok(frames)
    }

    /// Decode all packets sequentially and return every frame.
    /// More efficient than calling `decode_up_to` repeatedly because
    /// the decoder is only flushed once at the start.
    pub fn decode_all(
        &mut self,
        packets: &[&OwnedPacket],
    ) -> Result<Vec<Arc<Yuv420>>, DecodeError> {
        if packets.is_empty() {
            return Ok(vec![]);
        }

        self.decoder.flush();
        let mut frame = ffmpeg::util::frame::video::Video::empty();
        let mut frames: Vec<Arc<Yuv420>> = Vec::with_capacity(packets.len());

        for pkt in packets {
            let mut packet = ffmpeg::codec::packet::Packet::copy(&pkt.data);
            packet.set_pts(Some(pkt.pts));
            packet.set_dts(Some(pkt.dts));
            packet.set_duration(pkt.duration);
            if pkt.is_key {
                packet.set_flags(ffmpeg::codec::packet::Flags::KEY);
            }
            self.decoder.send_packet(&packet)?;
            while self.decoder.receive_frame(&mut frame).is_ok() {
                frames.push(Arc::new(copy_yuv_from_frame(&frame)));
            }
        }

        self.decoder.send_eof()?;
        while self.decoder.receive_frame(&mut frame).is_ok() {
            frames.push(Arc::new(copy_yuv_from_frame(&frame)));
        }

        Ok(frames)
    }
}

pub(crate) fn copy_yuv_from_frame(frame: &ffmpeg::util::frame::video::Video) -> Yuv420 {
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
