use std::path::Path;

use ffmpeg_next as ffmpeg;
use thiserror::Error;

use crate::codec::ir::{Frame, FrameType, MacroblockSize, Yuv420};

#[derive(Debug, Error)]
pub enum ImportError {
    #[error("ffmpeg error: {0}")]
    Ffmpeg(#[from] ffmpeg::Error),
    #[error("no video stream found in file")]
    NoVideoStream,
}

/// Decode every video frame from `path` into I-frames in the internal YUV420 IR.
///
/// Returns `(frames, width, height)`. All frames are stored as I-frames;
/// call the encoder's `encode_pframe` afterwards to compress into P-frames.
pub fn import_video(path: &Path) -> Result<(Vec<Frame>, u32, u32), ImportError> {
    ffmpeg::init()?;

    let mut ictx = ffmpeg::format::input(path)?;

    let stream = ictx
        .streams()
        .best(ffmpeg::media::Type::Video)
        .ok_or(ImportError::NoVideoStream)?;
    let stream_idx = stream.index();

    let codec_ctx =
        ffmpeg::codec::context::Context::from_parameters(stream.parameters())?;
    let mut decoder = codec_ctx.decoder().video()?;

    let width = decoder.width();
    let height = decoder.height();

    let mut scaler = ffmpeg::software::scaling::Context::get(
        decoder.format(),
        width,
        height,
        ffmpeg::format::Pixel::YUV420P,
        width,
        height,
        ffmpeg::software::scaling::Flags::BILINEAR,
    )?;

    let mut frames: Vec<Frame> = Vec::new();

    let push_decoded = |decoder: &mut ffmpeg::decoder::Video,
                            scaler: &mut ffmpeg::software::scaling::Context,
                            frames: &mut Vec<Frame>|
     -> Result<(), ImportError> {
        let mut raw = ffmpeg::util::frame::video::Video::empty();
        while decoder.receive_frame(&mut raw).is_ok() {
            let mut yuv = ffmpeg::util::frame::video::Video::empty();
            scaler.run(&raw, &mut yuv)?;
            let pts = raw.pts().unwrap_or(frames.len() as i64) as u64;
            frames.push(Frame {
                frame_type: FrameType::I,
                pts,
                mb_size: MacroblockSize::Mb16x16,
                reference: None,
                planes: Some(copy_yuv(&yuv, width, height)),
                motion_vectors: vec![],
                residuals: vec![],
            });
        }
        Ok(())
    };

    for (stream, packet) in ictx.packets() {
        if stream.index() != stream_idx {
            continue;
        }
        decoder.send_packet(&packet)?;
        push_decoded(&mut decoder, &mut scaler, &mut frames)?;
    }

    decoder.send_eof()?;
    push_decoded(&mut decoder, &mut scaler, &mut frames)?;

    Ok((frames, width, height))
}

/// Copy a YUV420P ffmpeg frame into our IR, stripping line padding.
fn copy_yuv(frame: &ffmpeg::util::frame::video::Video, width: u32, height: u32) -> Yuv420 {
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
