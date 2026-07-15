//! Regression test: the rendered file's duration is governed by the video
//! lane. An untrimmed audio clip left on the audio lane must not stretch a
//! one-second edit into a full-source-length output (the "8-minute export
//! from a 1-minute timeline" bug).
//!
//! Needs the ffmpeg CLI; skips (with a note) when it isn't available.

use std::path::Path;
use std::process::Command;

use rustjay_mosh::audio::{import_audio, render_audio_mix, AudioTimelineClip};
use rustjay_mosh::importer::import_video;
use rustjay_mosh::packet::OwnedPacket;
use rustjay_mosh::render::delivery::ExportPreset;
use rustjay_mosh::render::wysiwyg::bake_sequence_to_mp4;
use rustjay_mosh::ui::timeline_panel::{TimelineClip, TimelinePanel};

const FPS: u32 = 30;

fn ffmpeg_available() -> bool {
    Command::new(rustjay_mosh::bundled_ffmpeg())
        .arg("-version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Container duration in seconds, read back via ffmpeg-next.
fn container_duration_secs(path: &Path) -> f64 {
    let ictx = ffmpeg_next::format::input(&path).expect("open output");
    ictx.duration() as f64 / f64::from(ffmpeg_next::ffi::AV_TIME_BASE)
}

#[test]
fn render_duration_follows_video_lane_not_audio() {
    ffmpeg_next::init().unwrap();
    if !ffmpeg_available() {
        eprintln!("skipping: ffmpeg CLI not available");
        return;
    }

    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src.mp4");

    // 3 s test clip with an audio track.
    let synth = Command::new(rustjay_mosh::bundled_ffmpeg())
        .args([
            "-y",
            "-f", "lavfi", "-i", "testsrc2=duration=3:size=192x108:rate=30",
            "-f", "lavfi", "-i", "sine=frequency=440:duration=3",
            "-c:v", "libx264", "-pix_fmt", "yuv420p",
            "-c:a", "aac", "-shortest",
            src.to_str().unwrap(),
        ])
        .output()
        .expect("run ffmpeg");
    assert!(synth.status.success(), "synth clip failed: {}", String::from_utf8_lossy(&synth.stderr));

    let (clip, _preview) = import_video(&src, "src").expect("import video");
    let audio = import_audio(&src, "src", FPS).expect("import audio");

    // Timeline mirrors the bug report: video trimmed to 1 s, audio lane left
    // at its full source length.
    let mut panel = TimelinePanel::new();
    panel.clips.push(TimelineClip {
        id: 0,
        clip_idx: 0,
        name: "src".into(),
        frame_count: FPS as usize, // 1 s
        source_frame_count: clip.packets.len(),
        start_frame: 0,
        source_offset: 0,
        color: eframe::egui::Color32::WHITE,
        selected: false,
        drop_leading_keyframe: false,
        track: 1,
    });
    panel.audio_clips.push(AudioTimelineClip {
        audio_clip_idx: 0,
        start_frame: 0,
        frame_count: audio.peaks.len(), // full ~3 s
        source_offset: 0,
        fade_in_frames: 0,
        fade_out_frames: 0,
        selected: false,
    });

    // The rule under test: renders are sized by the video lane.
    assert_eq!(panel.video_frame_count(), FPS as usize);
    assert!(
        panel.total_frame_count() >= 2 * FPS as usize,
        "audio lane should exceed the video lane in this scenario (got {})",
        panel.total_frame_count()
    );

    // Render exactly as MoshApp::start_render does, with the video-lane length.
    let total_frames = panel.video_frame_count();
    let render_packets: Vec<(OwnedPacket, usize)> = clip
        .packets
        .iter()
        .take(total_frames)
        .enumerate()
        .map(|(i, p)| {
            (
                OwnedPacket {
                    data: p.data.clone(),
                    pts: i as i64,
                    dts: i as i64,
                    duration: 1,
                    is_key: p.is_key,
                },
                0usize,
            )
        })
        .collect();

    let video_temp = tmp.path().join("video.mp4");
    let emitted = bake_sequence_to_mp4(
        &render_packets,
        &[clip.codec_parameters.clone()],
        clip.width,
        clip.height,
        FPS,
        &video_temp,
    )
    .expect("bake");
    assert_eq!(emitted, total_frames);

    let audio_temp = tmp.path().join("audio.wav");
    render_audio_mix(&[audio], &panel.audio_clips, total_frames, FPS, &audio_temp)
        .expect("audio mix");

    let out = tmp.path().join("out.mp4");
    let args = ExportPreset::RawMosh.build_ffmpeg_args(
        video_temp.to_str().unwrap(),
        Some(audio_temp.to_str().unwrap()),
        out.to_str().unwrap(),
        FPS,
    );
    let mux = Command::new(rustjay_mosh::bundled_ffmpeg())
        .args(&args)
        .output()
        .expect("run mux");
    assert!(mux.status.success(), "mux failed: {}", String::from_utf8_lossy(&mux.stderr));

    let dur = container_duration_secs(&out);
    assert!(
        (0.8..1.3).contains(&dur),
        "output should run ~1 s (the video lane), got {dur:.2} s — \
         an untrimmed audio lane must not stretch the export"
    );
}
