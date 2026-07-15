//! Project save / load and shareable bundles.
//!
//! A project is a **self-contained bundle directory** (conventionally named
//! `*.rjmosh`) so that baked clips — which are synthesised in-app and have no
//! source file to relink to — survive a round-trip:
//!
//! ```text
//! my_project.rjmosh/
//!   project.json            manifest: timeline edits, fps, export preset, …
//!   media/clip_<i>.mp4       one per PacketClip (remuxed, no re-encode)
//!   audio/audio_<i>.wav      one per AudioClip (32-bit float)
//! ```
//!
//! [`save_bundle`] writes the directory; [`load_bundle`] reconstructs every
//! `PacketClip` / `AudioClip` by reading those media files back. [`collect_zip`]
//! packs the directory into a single `.zip` for sharing.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use eframe::egui::Color32;
use thiserror::Error;

use crate::audio::{self, AudioClip, AudioTimelineClip};
use crate::importer;
use crate::packet::PacketClip;
use crate::render::delivery::ExportPreset;
use crate::render::muxer::export_packets;
use crate::ui::timeline_panel::TimelineClip;

/// Bumped when the on-disk manifest layout changes incompatibly.
pub const FORMAT_VERSION: u32 = 1;

/// Canonical bundle directory extension.
pub const BUNDLE_EXT: &str = "rjmosh";

#[derive(Debug, Error)]
pub enum ProjectError {
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("zip error: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("wav error: {0}")]
    Wav(#[from] hound::Error),
    #[error("media import error: {0}")]
    Import(#[from] crate::importer::ImportError),
    #[error("audio import error: {0}")]
    Audio(#[from] crate::audio::AudioError),
    #[error("muxer error: {0}")]
    Mux(#[from] crate::render::muxer::MuxerError),
    #[error("unsupported project format version {0} (this build understands {FORMAT_VERSION})")]
    UnsupportedVersion(u32),
}

// ── On-disk manifest DTOs ───────────────────────────────────────────────────

#[derive(serde::Serialize, serde::Deserialize)]
pub struct ProjectManifest {
    pub format_version: u32,
    pub app_version: String,
    pub render_fps: u32,
    pub export_preset: ExportPreset,
    pub zoom: f32,
    pub playhead: i64,
    pub packet_clips: Vec<PacketClipMeta>,
    pub audio_clips: Vec<AudioClipMeta>,
    pub video_timeline: Vec<TimelineClipDto>,
    pub audio_timeline: Vec<AudioTimelineClipDto>,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct PacketClipMeta {
    pub id: u64,
    pub name: String,
    pub width: u32,
    pub height: u32,
    /// Path relative to the bundle root.
    pub media_file: String,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct AudioClipMeta {
    pub name: String,
    pub sample_rate: u32,
    pub channels: usize,
    pub media_file: String,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct TimelineClipDto {
    pub id: u64,
    pub clip_idx: usize,
    pub name: String,
    pub frame_count: usize,
    pub source_frame_count: usize,
    pub start_frame: i64,
    pub source_offset: usize,
    /// Premultiplied RGBA.
    pub color: [u8; 4],
    pub drop_leading_keyframe: bool,
    pub track: u8,
}

impl From<&TimelineClip> for TimelineClipDto {
    fn from(c: &TimelineClip) -> Self {
        Self {
            id: c.id,
            clip_idx: c.clip_idx,
            name: c.name.clone(),
            frame_count: c.frame_count,
            source_frame_count: c.source_frame_count,
            start_frame: c.start_frame,
            source_offset: c.source_offset,
            color: c.color.to_array(),
            drop_leading_keyframe: c.drop_leading_keyframe,
            track: c.track,
        }
    }
}

impl TimelineClipDto {
    fn to_clip(&self) -> TimelineClip {
        TimelineClip {
            id: self.id,
            clip_idx: self.clip_idx,
            name: self.name.clone(),
            frame_count: self.frame_count,
            source_frame_count: self.source_frame_count,
            start_frame: self.start_frame,
            source_offset: self.source_offset,
            color: Color32::from_rgba_premultiplied(
                self.color[0],
                self.color[1],
                self.color[2],
                self.color[3],
            ),
            selected: false,
            drop_leading_keyframe: self.drop_leading_keyframe,
            track: self.track,
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct AudioTimelineClipDto {
    pub audio_clip_idx: usize,
    pub start_frame: i64,
    pub frame_count: usize,
    pub source_offset: usize,
    pub fade_in_frames: usize,
    pub fade_out_frames: usize,
}

impl From<&AudioTimelineClip> for AudioTimelineClipDto {
    fn from(c: &AudioTimelineClip) -> Self {
        Self {
            audio_clip_idx: c.audio_clip_idx,
            start_frame: c.start_frame,
            frame_count: c.frame_count,
            source_offset: c.source_offset,
            fade_in_frames: c.fade_in_frames,
            fade_out_frames: c.fade_out_frames,
        }
    }
}

impl AudioTimelineClipDto {
    fn to_clip(&self) -> AudioTimelineClip {
        AudioTimelineClip {
            audio_clip_idx: self.audio_clip_idx,
            start_frame: self.start_frame,
            frame_count: self.frame_count,
            source_offset: self.source_offset,
            fade_in_frames: self.fade_in_frames,
            fade_out_frames: self.fade_out_frames,
            selected: false,
        }
    }
}

// ── Save ────────────────────────────────────────────────────────────────────

/// Everything the saver needs, borrowed from the live app state.
pub struct SaveRequest<'a> {
    pub packet_clips: &'a [PacketClip],
    pub audio_clips: &'a [AudioClip],
    pub video_timeline: &'a [TimelineClip],
    pub audio_timeline: &'a [AudioTimelineClip],
    pub render_fps: u32,
    pub export_preset: ExportPreset,
    pub zoom: f32,
    pub playhead: i64,
}

/// Write a self-contained project bundle to `dir` (created if absent).
pub fn save_bundle(dir: &Path, req: &SaveRequest) -> Result<(), ProjectError> {
    fs::create_dir_all(dir.join("media"))?;
    fs::create_dir_all(dir.join("audio"))?;

    for (i, clip) in req.packet_clips.iter().enumerate() {
        export_packets(
            &clip.packets,
            &dir.join(format!("media/clip_{i}.mp4")),
            &clip.codec_parameters,
            clip.time_base,
        )?;
    }
    for (i, clip) in req.audio_clips.iter().enumerate() {
        write_wav(&dir.join(format!("audio/audio_{i}.wav")), clip)?;
    }

    save_manifest(dir, req)
}

/// Write only `project.json`, assuming `dir`'s media files (written by an
/// earlier [`save_bundle`] for the same media pool) are still valid. Timeline
/// edits only change the manifest, so autosave uses this to avoid rewriting
/// gigabytes of unchanged media.
pub fn save_manifest(dir: &Path, req: &SaveRequest) -> Result<(), ProjectError> {
    let packet_metas = req
        .packet_clips
        .iter()
        .enumerate()
        .map(|(i, clip)| PacketClipMeta {
            id: clip.id,
            name: clip.name.clone(),
            width: clip.width,
            height: clip.height,
            media_file: format!("media/clip_{i}.mp4"),
        })
        .collect();

    let audio_metas = req
        .audio_clips
        .iter()
        .enumerate()
        .map(|(i, clip)| AudioClipMeta {
            name: clip.name.clone(),
            sample_rate: clip.sample_rate,
            channels: clip.channels,
            media_file: format!("audio/audio_{i}.wav"),
        })
        .collect();

    let manifest = ProjectManifest {
        format_version: FORMAT_VERSION,
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        render_fps: req.render_fps,
        export_preset: req.export_preset,
        zoom: req.zoom,
        playhead: req.playhead,
        packet_clips: packet_metas,
        audio_clips: audio_metas,
        video_timeline: req.video_timeline.iter().map(TimelineClipDto::from).collect(),
        audio_timeline: req
            .audio_timeline
            .iter()
            .map(AudioTimelineClipDto::from)
            .collect(),
    };

    let json = serde_json::to_string_pretty(&manifest)?;
    fs::write(dir.join("project.json"), json)?;
    Ok(())
}

fn write_wav(path: &Path, clip: &AudioClip) -> Result<(), ProjectError> {
    let spec = hound::WavSpec {
        channels: clip.channels.max(1) as u16,
        sample_rate: clip.sample_rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut writer = hound::WavWriter::create(path, spec)?;
    for &s in &clip.samples {
        writer.write_sample(s)?;
    }
    writer.finalize()?;
    Ok(())
}

// ── Load ────────────────────────────────────────────────────────────────────

/// Fully reconstructed project state, ready to drop into the app.
pub struct LoadedProject {
    pub packet_clips: Vec<PacketClip>,
    pub audio_clips: Vec<AudioClip>,
    pub video_timeline: Vec<TimelineClip>,
    pub audio_timeline: Vec<AudioTimelineClip>,
    pub render_fps: u32,
    pub export_preset: ExportPreset,
    pub zoom: f32,
    pub playhead: i64,
}

/// Read a project bundle directory back into full app state.
pub fn load_bundle(dir: &Path) -> Result<LoadedProject, ProjectError> {
    let json = fs::read_to_string(dir.join("project.json"))?;
    let manifest: ProjectManifest = serde_json::from_str(&json)?;
    if manifest.format_version > FORMAT_VERSION {
        return Err(ProjectError::UnsupportedVersion(manifest.format_version));
    }

    let mut packet_clips = Vec::with_capacity(manifest.packet_clips.len());
    for meta in &manifest.packet_clips {
        let clip = importer::read_clip_from_mp4(&dir.join(&meta.media_file), &meta.name, meta.id)?;
        packet_clips.push(clip);
    }

    let mut audio_clips = Vec::with_capacity(manifest.audio_clips.len());
    for meta in &manifest.audio_clips {
        let clip = audio::read_audio_clip_from_wav(
            &dir.join(&meta.media_file),
            &meta.name,
            manifest.render_fps.max(1),
        )?;
        audio_clips.push(clip);
    }

    Ok(LoadedProject {
        packet_clips,
        audio_clips,
        video_timeline: manifest.video_timeline.iter().map(|d| d.to_clip()).collect(),
        audio_timeline: manifest.audio_timeline.iter().map(|d| d.to_clip()).collect(),
        render_fps: manifest.render_fps,
        export_preset: manifest.export_preset,
        zoom: manifest.zoom,
        playhead: manifest.playhead,
    })
}

// ── Collect / share (zip) ────────────────────────────────────────────────────

/// Pack a bundle directory into a single `.zip`, preserving the bundle folder
/// as the archive's top-level directory.
pub fn collect_zip(bundle_dir: &Path, zip_path: &Path) -> Result<(), ProjectError> {
    let file = fs::File::create(zip_path)?;
    let mut zip = zip::ZipWriter::new(file);
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    let root = bundle_dir
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("project")
        .to_string();
    add_dir_to_zip(&mut zip, bundle_dir, &root, &opts)?;
    zip.finish()?;
    Ok(())
}

fn add_dir_to_zip(
    zip: &mut zip::ZipWriter<fs::File>,
    dir: &Path,
    prefix: &str,
    opts: &zip::write::SimpleFileOptions,
) -> Result<(), ProjectError> {
    let mut entries: Vec<_> = fs::read_dir(dir)?.collect::<Result<_, _>>()?;
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let zip_path = format!("{prefix}/{name}");
        if path.is_dir() {
            add_dir_to_zip(zip, &path, &zip_path, opts)?;
        } else {
            zip.start_file(zip_path, *opts)?;
            let mut f = fs::File::open(&path)?;
            let mut buf = Vec::new();
            f.read_to_end(&mut buf)?;
            zip.write_all(&buf)?;
        }
    }
    Ok(())
}

// ── App config: recent projects + autosave location ──────────────────────────

/// Per-user config directory for this app (`<config>/rustjay-mosh`).
pub fn config_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("rustjay-mosh"))
}

fn recent_path() -> Option<PathBuf> {
    config_dir().map(|d| d.join("recent.json"))
}

/// Location of the autosave recovery bundle.
pub fn autosave_path() -> Option<PathBuf> {
    config_dir().map(|d| d.join("autosave").join("recovery.rjmosh"))
}

/// Load the most-recently-used project paths (newest first), pruning any that
/// no longer exist on disk.
pub fn load_recent() -> Vec<PathBuf> {
    let Some(path) = recent_path() else { return vec![] };
    let Ok(json) = fs::read_to_string(&path) else { return vec![] };
    let list: Vec<String> = serde_json::from_str(&json).unwrap_or_default();
    list.into_iter()
        .map(PathBuf::from)
        .filter(|p| p.exists())
        .collect()
}

/// Record `path` as the most-recently-used project (deduped, capped at 10).
pub fn push_recent(path: &Path) {
    let Some(store) = recent_path() else { return };
    let mut list = load_recent();
    list.retain(|p| p != path);
    list.insert(0, path.to_path_buf());
    list.truncate(10);
    let strings: Vec<String> = list.iter().map(|p| p.to_string_lossy().into_owned()).collect();
    if let Some(parent) = store.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(&strings) {
        let _ = fs::write(&store, json);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_clip() -> TimelineClip {
        TimelineClip {
            id: 7,
            clip_idx: 2,
            name: "shot".into(),
            frame_count: 120,
            source_frame_count: 200,
            start_frame: 30,
            source_offset: 5,
            color: Color32::from_rgb(200, 30, 30),
            selected: true, // must NOT survive a round-trip
            drop_leading_keyframe: true,
            track: 1,
        }
    }

    #[test]
    fn timeline_clip_dto_round_trips_edits_and_color() {
        let dto = TimelineClipDto::from(&sample_clip());
        let back = dto.to_clip();
        assert_eq!(back.id, 7);
        assert_eq!(back.clip_idx, 2);
        assert_eq!(back.frame_count, 120);
        assert_eq!(back.source_offset, 5);
        assert!(back.drop_leading_keyframe);
        assert_eq!(back.track, 1);
        assert_eq!(back.color, Color32::from_rgb(200, 30, 30));
        // Selection is transient editor state, never persisted.
        assert!(!back.selected);
    }

    #[test]
    fn manifest_json_round_trips() {
        let manifest = ProjectManifest {
            format_version: FORMAT_VERSION,
            app_version: "test".into(),
            render_fps: 24,
            export_preset: ExportPreset::YouTube4K,
            zoom: 6.5,
            playhead: 42,
            packet_clips: vec![PacketClipMeta {
                id: 1,
                name: "a".into(),
                width: 1920,
                height: 1080,
                media_file: "media/clip_0.mp4".into(),
            }],
            audio_clips: vec![],
            video_timeline: vec![TimelineClipDto::from(&sample_clip())],
            audio_timeline: vec![AudioTimelineClipDto {
                audio_clip_idx: 0,
                start_frame: 10,
                frame_count: 50,
                source_offset: 0,
                fade_in_frames: 3,
                fade_out_frames: 4,
            }],
        };

        let json = serde_json::to_string_pretty(&manifest).unwrap();
        let back: ProjectManifest = serde_json::from_str(&json).unwrap();

        assert_eq!(back.render_fps, 24);
        assert_eq!(back.export_preset, ExportPreset::YouTube4K);
        assert_eq!(back.playhead, 42);
        assert_eq!(back.packet_clips.len(), 1);
        assert_eq!(back.packet_clips[0].media_file, "media/clip_0.mp4");
        assert_eq!(back.video_timeline.len(), 1);
        assert_eq!(back.video_timeline[0].id, 7);
        assert_eq!(back.audio_timeline[0].fade_out_frames, 4);
    }
}
