use std::path::Path;

use ffmpeg_next as ffmpeg;
use thiserror::Error;

use crate::packet::OwnedPacket;

#[derive(Debug, Error)]
pub enum MuxerError {
    #[error("ffmpeg error: {0}")]
    Ffmpeg(#[from] ffmpeg::Error),
    #[error("no video stream in output")]
    NoVideoStream,
}

/// Remux a sequence of `OwnedPacket`s into an MP4 file without re-encoding.
///
/// `time_base` must match the timebase used by the packet timestamps.
/// `codec_params` provides the stream codec configuration.
pub fn export_packets(
    packets: &[OwnedPacket],
    output_path: &Path,
    codec_params: &ffmpeg::codec::Parameters,
    time_base: ffmpeg::Rational,
) -> Result<(), MuxerError> {
    let mut octx = ffmpeg::format::output(&output_path)?;

    let mut ost = octx.add_stream(ffmpeg::encoder::find(ffmpeg::codec::Id::None))?;
    ost.set_parameters(codec_params.clone());
    ost.set_time_base(time_base);

    // Reset codec_tag so the MP4 muxer picks a compatible one.
    unsafe {
        (*ost.parameters().as_mut_ptr()).codec_tag = 0;
    }

    let ost_index = ost.index();

    octx.write_header()?;

    for pkt in packets {
        let mut packet = ffmpeg::codec::packet::Packet::copy(&pkt.data);
        if pkt.is_key {
            packet.set_flags(ffmpeg::codec::packet::Flags::KEY);
        }

        packet.set_pts(Some(pkt.pts));
        packet.set_dts(Some(pkt.dts));
        packet.set_duration(pkt.duration);
        packet.set_stream(ost_index);
        packet.set_position(-1);

        packet.write_interleaved(&mut octx)?;
    }

    octx.write_trailer()?;
    Ok(())
}
