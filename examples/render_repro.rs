// Runnable check for the WYSIWYG render path: load a project bundle, build
// the two-track packet sequence exactly like MoshApp::start_render, bake it
// to MP4, and assert every timeline frame survived into the output.
//
// Usage: cargo run --release --example render_repro [bundle_dir]
use std::path::Path;

use rustjay_mosh::packet::{OwnedPacket, PacketClip};
use rustjay_mosh::project::load_bundle;
use rustjay_mosh::render::wysiwyg::bake_sequence_to_mp4;
use rustjay_mosh::ui::timeline_panel::TimelineClip;

// Mirror of the private resolver in ui::app (top track wins).
fn resolve_packet_at<'a>(
    clips: &'a [TimelineClip],
    packet_clips: &'a [PacketClip],
    f: i64,
) -> Option<(&'a OwnedPacket, usize)> {
    for track in 0..=1u8 {
        for clip in clips {
            if clip.track != track {
                continue;
            }
            if f < clip.start_frame || f >= clip.end_frame() {
                continue;
            }
            let drop_skip = if clip.drop_leading_keyframe { 1 } else { 0 };
            let src_idx = clip.source_offset + drop_skip + (f - clip.start_frame) as usize;
            match packet_clips
                .get(clip.clip_idx)
                .and_then(|pc| pc.packets.get(src_idx))
            {
                Some(p) => return Some((p, clip.clip_idx)),
                None => continue,
            }
        }
    }
    None
}

fn main() {
    ffmpeg_next::init().unwrap();
    let arg = std::env::args().nth(1);
    let bundle = Path::new(arg.as_deref().unwrap_or("projects/TEST_001.rjmosh"));
    let proj = load_bundle(bundle).expect("load bundle");

    let total: i64 = proj
        .video_timeline
        .iter()
        .map(|c| c.end_frame())
        .max()
        .unwrap_or(0);

    let mut seq: Vec<(OwnedPacket, usize)> = Vec::new();
    for f in 0..total {
        if let Some((pkt, owner)) = resolve_packet_at(&proj.video_timeline, &proj.packet_clips, f) {
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
    }
    println!("timeline frames: {total}, sequence packets: {}", seq.len());

    let params: Vec<_> = proj
        .packet_clips
        .iter()
        .map(|c| c.codec_parameters.clone())
        .collect();
    let (w, h) = (proj.packet_clips[seq[0].1].width, proj.packet_clips[seq[0].1].height);

    let out = Path::new("/tmp/repro_wysiwyg.mp4");
    let emitted =
        bake_sequence_to_mp4(&seq, &params, w, h, proj.render_fps, out).expect("bake render");
    assert_eq!(emitted, seq.len(), "every timeline frame must be emitted");

    // Read the output back and confirm a decoder gets every frame out of it.
    let clip = rustjay_mosh::importer::read_clip_from_mp4(out, "check", 0).expect("read back");
    let refs: Vec<&OwnedPacket> = clip.packets.iter().collect();
    let mut dec =
        rustjay_mosh::preview::decoder::PacketDecoder::new(&clip.codec_parameters).unwrap();
    let frames = dec.decode_all(&refs).expect("decode output");
    assert_eq!(frames.len(), seq.len(), "output must decode losslessly");
    println!("OK: {} packets → {} baked+decodable frames → {}", seq.len(), frames.len(), out.display());
}
