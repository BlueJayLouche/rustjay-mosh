//! Regression test: a moshed (keyframe-dropped) baked clip must stay decodable
//! against an *imported* clip's SPS/PPS. If the importer and the bake encoder
//! use different x264 configs, the baked P-frames misparse under the source's
//! headers, the decoder refuses them, and the WYSIWYG render holds one frame
//! for the whole span — "the video just holds the first frame".
//!
//! Needs the ffmpeg CLI; skips (with a note) when it isn't available.

use std::path::Path;
use std::process::Command;
use std::sync::atomic::AtomicBool;

use rustjay_mosh::bake::{bake_segment, BendMode, Effect, SortDir, SortKey};
use rustjay_mosh::importer::import_video;
use rustjay_mosh::packet::OwnedPacket;
use rustjay_mosh::preview::decoder::PacketDecoder;
use rustjay_mosh::render::wysiwyg::bake_sequence_to_mp4;

fn ffmpeg_available() -> bool {
    Command::new(rustjay_mosh::bundled_ffmpeg())
        .arg("-version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn mean_abs_diff(a: &[u8], b: &[u8]) -> f64 {
    a.iter().zip(b).map(|(x, y)| (*x as f64 - *y as f64).abs()).sum::<f64>() / a.len() as f64
}

#[test]
fn moshed_bake_over_imported_clip_keeps_motion() {
    ffmpeg_next::init().unwrap();
    ffmpeg_next::util::log::set_level(ffmpeg_next::util::log::Level::Fatal);
    if !ffmpeg_available() {
        eprintln!("skipping: ffmpeg CLI not available");
        return;
    }

    let tmp = tempfile::tempdir().unwrap();
    let src_file = tmp.path().join("src.mp4");
    // Moving synthetic source, 2 s @ 30 fps.
    let synth = Command::new(rustjay_mosh::bundled_ffmpeg())
        .args([
            "-y",
            "-f", "lavfi", "-i", "testsrc2=duration=2:size=640x360:rate=30",
            "-c:v", "libx264", "-pix_fmt", "yuv420p",
            src_file.to_str().unwrap(),
        ])
        .output()
        .expect("run ffmpeg");
    assert!(synth.status.success(), "synth failed: {}", String::from_utf8_lossy(&synth.stderr));

    // Real import path (CLI transcode → 1920x1080 long-GOP stream).
    let (source, _) = import_video(Path::new(&src_file), "src").expect("import");
    let n = source.packets.len();
    assert!(n >= 55, "unexpected import length {n}");

    // Real bake path (in-process encoder).
    let cancel = AtomicBool::new(false);
    let effect = Effect::Bend(BendMode::PixelSort {
        dir: SortDir::Horizontal,
        key: SortKey::Luma,
        lo: 64,
        hi: 192,
        reverse: false,
    });
    let baked = bake_segment(&source, 20, 20, effect, 30, &mut |_| {}, &cancel).expect("bake");

    // Timeline shape from the real app: baked overlay moshed onto the source
    // (leading keyframe dropped), source resumes mid-GOP afterwards.
    let mut seq: Vec<(OwnedPacket, usize)> = Vec::new();
    for f in 0..55usize {
        let (pkt, owner) = if (20..39).contains(&f) {
            (&baked.packets[f - 20 + 1], 1usize)
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
    let out_path = tmp.path().join("wys.mp4");
    let emitted =
        bake_sequence_to_mp4(&seq, &params, source.width, source.height, 30, &out_path)
            .expect("wysiwyg");
    assert_eq!(emitted, 55);

    let clip = rustjay_mosh::importer::read_clip_from_mp4(&out_path, "check", 0).expect("read");
    let refs: Vec<&OwnedPacket> = clip.packets.iter().collect();
    let mut dec = PacketDecoder::new(&clip.codec_parameters).unwrap();
    let out = dec.decode_all(&refs).expect("decode");
    assert_eq!(out.len(), 55);

    // Inside the moshed baked span the output must not be a held frame.
    let held = (23..38).all(|i| mean_abs_diff(&out[i].y, &out[22].y) < 0.5);
    assert!(
        !held,
        "moshed baked span rendered as a single held frame — bake/import \
         encoder configs are producing incompatible SPS/PPS"
    );
}
