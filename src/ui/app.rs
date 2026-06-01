use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::{mpsc, Arc};
use std::time::Instant;

use eframe::egui::{self, CursorIcon};
use eframe::egui_wgpu;

use crate::audio::{import_audio, AudioClip, AudioTimelineClip};
use crate::bake::{BendMode, CompressRegion, Effect, bake_segment};
use crate::codec::ir::Yuv420;
use crate::importer::import_video;
use crate::packet::{OwnedPacket, PacketClip};
use crate::preview::decoder::PacketDecoder;
use crate::project::{self, LoadedProject, SaveRequest};
use crate::render::delivery::ExportPreset;
use crate::render::muxer::export_packets;
use crate::ui::preview::{YuvPreviewCallback, YuvResources};
use crate::ui::timeline_panel::{next_clip_color, PoolDragPayload, TimelineClip, TimelinePanel};

// ── Background messages ────────────────────────────────────────────────────────

enum ImportResult {
    Video { name: String, packet_clip: PacketClip, audio_clip: Option<AudioClip> },
    Audio { name: String, audio_clip: AudioClip },
}

type RenderResult = Result<String, String>;

/// A preview-decode request sent to the background worker:
/// `(owning pool clip index, that clip's codec parameters, packet slice,
/// target index within the slice, absolute timeline frame)`.
///
/// The clip's own parameters travel with the request because clips carry
/// different H.264 SPS/PPS (different sources/encoders); the worker (re)creates
/// its decoder from them whenever the owning clip changes, so any clip decodes
/// correctly regardless of timeline order.
type PreviewRequest = (
    usize,
    ffmpeg_next::codec::Parameters,
    Vec<OwnedPacket>,
    usize,
    usize,
);

struct BakeDone {
    clip: PacketClip,
    source_name: String,
    /// Id of the source `TimelineClip` the user baked from.
    source_tl_clip_id: u64,
    /// First source packet included in the bake (source-packet coordinates).
    /// Used to align the baked clip over the source region on V1.
    source_start: usize,
}

enum BakeMsg {
    Progress(f32),
    Done(Result<BakeDone, String>),
}

/// A path chosen in a native dialog, tagged with what the user intends to do.
enum ProjectRequest {
    SaveAs(PathBuf),
    Open(PathBuf),
    Collect(PathBuf),
    ExportAll(PathBuf),
}

/// Result of an async project I/O operation, applied on the main thread.
enum ProjectMsg {
    Loaded(Box<LoadedProject>, PathBuf),
    Collected(PathBuf),
    ExportedAll(PathBuf),
    Progress(String),
    Error(String),
}

/// Snapshot of the editable timeline state for undo/redo. Holds only the light
/// edit decisions — never the heavy media (those live in `packet_clips` /
/// `audio_clips` and are not mutated by undoable operations).
#[derive(Clone, PartialEq)]
struct EditSnapshot {
    clips: Vec<TimelineClip>,
    audio_clips: Vec<AudioTimelineClip>,
}

// ── App ───────────────────────────────────────────────────────────────────────

pub struct MoshApp {
    packet_clips: Vec<PacketClip>,
    audio_clips: Vec<AudioClip>,
    preview_cache: Option<(usize, Arc<Yuv420>)>,
    /// Request channel to the background preview decode thread.
    /// Payload: (packets to decode, target index within that slice, absolute timeline frame).
    preview_req_tx: Option<mpsc::SyncSender<PreviewRequest>>,
    /// Results from the background decode thread: (absolute timeline frame, decoded image).
    preview_res_rx: mpsc::Receiver<(usize, Arc<Yuv420>)>,
    /// Sender half stored so we can clone it into the worker when it spawns.
    preview_res_tx: mpsc::Sender<(usize, Arc<Yuv420>)>,
    /// Set when a try_send failed (worker busy); schedule a retry repaint.
    preview_dirty: bool,

    timeline: TimelinePanel,
    color_idx: usize,
    clip_uid: u64,

    file_rx: mpsc::Receiver<PathBuf>,
    file_tx: mpsc::SyncSender<PathBuf>,

    import_rx: mpsc::Receiver<Result<ImportResult, String>>,
    import_tx: mpsc::SyncSender<Result<ImportResult, String>>,

    render_rx: mpsc::Receiver<PathBuf>,
    render_tx: mpsc::SyncSender<PathBuf>,
    render_result_rx: mpsc::Receiver<RenderResult>,
    render_result_tx: mpsc::SyncSender<RenderResult>,

    is_rendering: bool,
    status: String,
    render_fps: u32,
    export_preset: ExportPreset,

    // ── Glitch dialog state ───────────────────────────────────────────────
    show_bend_dialog: bool,
    bend_mode: usize,       // 0=ReverseScanlines, 1=Echo, 2=Bitcrush, 3=ByteSwap, 4=Xor, 5=Noise
    bend_duration: usize,   // 0=1frame, 1=±5, 2=±15, 3=whole
    bend_echo_delay: usize,
    bend_echo_mix: f32,
    bend_bitcrush_bits: u8,
    bend_byteswap_stride: usize,
    bend_xor_mask: u8,
    bend_noise_amount: u8,

    show_compress_dialog: bool,
    compress_x: u32,
    compress_y: u32,
    compress_w: u32,
    compress_h: u32,
    compress_quality: u8,
    compress_duration: usize,
    compress_dialog_clip_id: Option<u64>,

    // ── Bake job state ────────────────────────────────────────────────────
    bake_rx: mpsc::Receiver<BakeMsg>,
    bake_tx: mpsc::SyncSender<BakeMsg>,
    bake_in_progress: bool,
    bake_progress: f32,
    bake_cancel: Arc<AtomicBool>,

    // ── Project I/O state ─────────────────────────────────────────────────
    /// The current bundle directory, once the project has been saved/opened.
    project_path: Option<PathBuf>,
    /// True when there are unsaved edits.
    project_dirty: bool,
    recent_projects: Vec<PathBuf>,
    /// A recovery bundle left by autosave, if one was found on startup.
    recovery_available: Option<PathBuf>,
    proj_req_rx: mpsc::Receiver<ProjectRequest>,
    proj_req_tx: mpsc::SyncSender<ProjectRequest>,
    proj_msg_rx: mpsc::Receiver<ProjectMsg>,
    proj_msg_tx: mpsc::Sender<ProjectMsg>,
    proj_busy: bool,
    last_autosave: Instant,
    /// Set when "New Project" needs to confirm discarding unsaved changes.
    show_new_confirm: bool,
    /// Set when a save was requested via the confirm's "Save first…" — start a
    /// new project once that save succeeds.
    new_after_save: bool,

    // ── Undo / redo ───────────────────────────────────────────────────────
    undo_stack: Vec<EditSnapshot>,
    redo_stack: Vec<EditSnapshot>,
    /// The last committed editing state, used to detect new edit points.
    last_snapshot: EditSnapshot,
}

impl MoshApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        if let Some(ws) = cc.wgpu_render_state.as_ref() {
            ws.renderer
                .write()
                .callback_resources
                .insert(YuvResources::new(&ws.device, ws.target_format));
        }
        let (file_tx, file_rx) = mpsc::sync_channel(1);
        let (import_tx, import_rx) = mpsc::sync_channel(4);
        let (render_tx, render_rx) = mpsc::sync_channel(1);
        let (render_result_tx, render_result_rx) = mpsc::sync_channel(1);
        let (bake_tx, bake_rx) = mpsc::sync_channel(64);
        let (preview_res_tx, preview_res_rx) = mpsc::channel();
        let (proj_req_tx, proj_req_rx) = mpsc::sync_channel(1);
        let (proj_msg_tx, proj_msg_rx) = mpsc::channel();

        // Offer crash recovery if a previous session left an autosave bundle.
        let recovery_available = project::autosave_path()
            .filter(|p| p.join("project.json").exists());

        Self {
            packet_clips: vec![],
            audio_clips: vec![],
            preview_cache: None,
            preview_req_tx: None,
            preview_res_rx,
            preview_res_tx,
            preview_dirty: false,
            timeline: TimelinePanel::new(),
            color_idx: 0,
            clip_uid: 0,
            file_rx,
            file_tx,
            import_rx,
            import_tx,
            render_rx,
            render_tx,
            render_result_rx,
            render_result_tx,
            is_rendering: false,
            status: "Open a video or audio file to begin.".into(),
            render_fps: 30,
            export_preset: ExportPreset::RawMosh,

            show_bend_dialog: false,
            bend_mode: 0,
            bend_duration: 0,
            bend_echo_delay: 4,
            bend_echo_mix: 0.5,
            bend_bitcrush_bits: 4,
            bend_byteswap_stride: 4,
            bend_xor_mask: 0xFF,
            bend_noise_amount: 32,

            show_compress_dialog: false,
            compress_x: 0,
            compress_y: 0,
            compress_w: 1920,
            compress_h: 1080,
            compress_quality: 15,
            compress_duration: 0,
            compress_dialog_clip_id: None,

            bake_rx,
            bake_tx,
            bake_in_progress: false,
            bake_progress: 0.0,
            bake_cancel: Arc::new(AtomicBool::new(false)),

            project_path: None,
            project_dirty: false,
            recent_projects: project::load_recent(),
            recovery_available,
            proj_req_rx,
            proj_req_tx,
            proj_msg_rx,
            proj_msg_tx,
            proj_busy: false,
            last_autosave: Instant::now(),
            show_new_confirm: false,
            new_after_save: false,

            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            last_snapshot: EditSnapshot { clips: Vec::new(), audio_clips: Vec::new() },
        }
    }

    // ── Project I/O ───────────────────────────────────────────────────────────

    /// Snapshot the current editable timeline state.
    fn snapshot(&self) -> EditSnapshot {
        EditSnapshot {
            clips: self.timeline.clips.clone(),
            audio_clips: self.timeline.audio_clips.clone(),
        }
    }

    /// Spawn (once) the background preview-decode worker. Each request carries
    /// the codec parameters of the clip that owns the slice's leading keyframe;
    /// the worker rebuilds its decoder whenever that clip changes, so clips with
    /// different SPS/PPS all decode correctly regardless of timeline order.
    fn ensure_preview_worker(&mut self) {
        if self.preview_req_tx.is_some() || self.packet_clips.is_empty() {
            return;
        }
        let (req_tx, req_rx) = mpsc::sync_channel::<PreviewRequest>(1);
        self.preview_req_tx = Some(req_tx);
        let res_tx = self.preview_res_tx.clone();
        std::thread::spawn(move || {
            // (owning clip index, decoder) — rebuilt when the owning clip changes.
            let mut current: Option<(usize, PacketDecoder)> = None;
            while let Ok((clip_idx, params, packets, target_in_slice, abs_target)) = req_rx.recv() {
                let need_new = !matches!(&current, Some((idx, _)) if *idx == clip_idx);
                if need_new {
                    current = PacketDecoder::new(&params).ok().map(|d| (clip_idx, d));
                }
                if let Some((_, decoder)) = current.as_mut() {
                    let refs: Vec<&OwnedPacket> = packets.iter().collect();
                    if let Ok(yuv) = decoder.decode_up_to(&refs, target_in_slice) {
                        let _ = res_tx.send((abs_target, yuv));
                    }
                }
            }
        });
    }

    /// Mark the project as having unsaved edits.
    fn mark_dirty(&mut self) {
        self.project_dirty = true;
    }

    fn project_title(&self) -> String {
        let name = self
            .project_path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "untitled".into());
        if self.project_dirty {
            format!("{name} *")
        } else {
            name
        }
    }

    /// Build a borrowed save request over the current live state.
    fn save_request(&self) -> SaveRequest<'_> {
        SaveRequest {
            packet_clips: &self.packet_clips,
            audio_clips: &self.audio_clips,
            video_timeline: &self.timeline.clips,
            audio_timeline: &self.timeline.audio_clips,
            render_fps: self.render_fps,
            export_preset: self.export_preset,
            zoom: self.timeline.zoom,
            playhead: self.timeline.playhead,
        }
    }

    /// Save synchronously to `dir`. Media writes are usually quick (remux only),
    /// so this runs inline; the UI shows the result via `status`.
    fn save_to(&mut self, dir: PathBuf) {
        let req = self.save_request();
        match project::save_bundle(&dir, &req) {
            Ok(()) => {
                self.project_path = Some(dir.clone());
                self.project_dirty = false;
                project::push_recent(&dir);
                self.recent_projects = project::load_recent();
                // A clean save supersedes any stale recovery snapshot.
                if let Some(rec) = project::autosave_path() {
                    let _ = std::fs::remove_dir_all(&rec);
                }
                self.recovery_available = None;
                self.status = format!("Saved project → {}", dir.display());
                // If this save was requested to clear the way for a new project,
                // start it now that the work is safely on disk.
                if self.new_after_save {
                    self.new_project();
                }
            }
            Err(e) => {
                self.new_after_save = false;
                self.status = format!("Save failed: {e}");
            }
        }
    }

    /// Ctrl+S: save in place if we have a path, else prompt for one.
    fn save_or_prompt(&mut self, ctx: &egui::Context) {
        if let Some(dir) = self.project_path.clone() {
            self.save_to(dir);
        } else {
            self.prompt_save_as(ctx);
        }
    }

    /// True when the project holds work that would be lost by starting over.
    fn has_unsaved_work(&self) -> bool {
        self.project_dirty
            && !(self.packet_clips.is_empty()
                && self.audio_clips.is_empty()
                && self.timeline.clips.is_empty()
                && self.timeline.audio_clips.is_empty())
    }

    /// Start a new project — confirm first if there are unsaved changes.
    fn request_new_project(&mut self) {
        if self.has_unsaved_work() {
            self.show_new_confirm = true;
        } else {
            self.new_project();
        }
    }

    /// Reset all editor state to an empty project.
    fn new_project(&mut self) {
        self.show_new_confirm = false;
        self.new_after_save = false;
        self.packet_clips.clear();
        self.audio_clips.clear();
        self.timeline.clips.clear();
        self.timeline.audio_clips.clear();
        self.timeline.clear_selection();
        self.timeline.set_next_id(0);
        self.timeline.playhead = 0;
        self.timeline.zoom = 4.0;

        self.clip_uid = 0;
        self.color_idx = 0;
        self.render_fps = 30;
        self.export_preset = ExportPreset::RawMosh;

        // Tear down the preview worker so the next import seeds a fresh one.
        self.preview_req_tx = None;
        self.preview_cache = None;

        self.undo_stack.clear();
        self.redo_stack.clear();
        self.last_snapshot = self.snapshot();

        self.project_path = None;
        self.project_dirty = false;
        self.status = "New project.".into();
    }

    fn prompt_save_as(&self, ctx: &egui::Context) {
        let tx = self.proj_req_tx.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            if let Some(p) = rfd::FileDialog::new()
                .add_filter("rustjay-mosh project", &[project::BUNDLE_EXT])
                .set_file_name(format!("untitled.{}", project::BUNDLE_EXT))
                .save_file()
            {
                let _ = tx.send(ProjectRequest::SaveAs(p));
                ctx.request_repaint();
            }
        });
    }

    fn prompt_open(&self, ctx: &egui::Context) {
        let tx = self.proj_req_tx.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            if let Some(p) = rfd::FileDialog::new().pick_folder() {
                let _ = tx.send(ProjectRequest::Open(p));
                ctx.request_repaint();
            }
        });
    }

    fn prompt_collect(&self, ctx: &egui::Context) {
        let tx = self.proj_req_tx.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            if let Some(p) = rfd::FileDialog::new()
                .add_filter("Zip archive", &["zip"])
                .set_file_name("project_bundle.zip")
                .save_file()
            {
                let _ = tx.send(ProjectRequest::Collect(p));
                ctx.request_repaint();
            }
        });
    }

    fn prompt_export_all(&self, ctx: &egui::Context) {
        let tx = self.proj_req_tx.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            if let Some(p) = rfd::FileDialog::new().pick_folder() {
                let _ = tx.send(ProjectRequest::ExportAll(p));
                ctx.request_repaint();
            }
        });
    }

    /// Dispatch a dialog result to the appropriate async worker.
    fn handle_project_request(&mut self, req: ProjectRequest, ctx: &egui::Context) {
        match req {
            ProjectRequest::SaveAs(dir) => self.save_to(dir),
            ProjectRequest::Open(dir) => self.start_open(dir, ctx),
            ProjectRequest::Collect(zip) => self.start_collect(zip, ctx),
            ProjectRequest::ExportAll(dir) => self.start_export_all(dir, ctx),
        }
    }

    /// Load a project bundle on a background thread.
    fn start_open(&mut self, dir: PathBuf, ctx: &egui::Context) {
        self.proj_busy = true;
        self.status = format!("Opening {}…", dir.display());
        let tx = self.proj_msg_tx.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let msg = match project::load_bundle(&dir) {
                Ok(lp) => ProjectMsg::Loaded(Box::new(lp), dir),
                Err(e) => ProjectMsg::Error(format!("Open failed: {e}")),
            };
            let _ = tx.send(msg);
            ctx.request_repaint();
        });
    }

    /// Build a fresh bundle in a temp dir and zip it — works even if the
    /// project has never been saved to a permanent location.
    fn start_collect(&mut self, zip_path: PathBuf, ctx: &egui::Context) {
        self.proj_busy = true;
        self.status = "Collecting files into a shareable zip…".into();
        // The save request borrows self; snapshot owned copies for the thread.
        let packet_clips = self.packet_clips.clone();
        let audio_clips = self.audio_clips.clone();
        let video_timeline = self.timeline.clips.clone();
        let audio_timeline = self.timeline.audio_clips.clone();
        let render_fps = self.render_fps;
        let export_preset = self.export_preset;
        let zoom = self.timeline.zoom;
        let playhead = self.timeline.playhead;
        let tx = self.proj_msg_tx.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let msg = (|| -> Result<PathBuf, String> {
                let tmp = tempfile::tempdir().map_err(|e| e.to_string())?;
                let bundle = tmp.path().join(
                    zip_path
                        .file_stem()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_else(|| "project".into()),
                );
                let req = SaveRequest {
                    packet_clips: &packet_clips,
                    audio_clips: &audio_clips,
                    video_timeline: &video_timeline,
                    audio_timeline: &audio_timeline,
                    render_fps,
                    export_preset,
                    zoom,
                    playhead,
                };
                project::save_bundle(&bundle, &req).map_err(|e| e.to_string())?;
                project::collect_zip(&bundle, &zip_path).map_err(|e| e.to_string())?;
                Ok(zip_path)
            })();
            let _ = tx.send(match msg {
                Ok(p) => ProjectMsg::Collected(p),
                Err(e) => ProjectMsg::Error(format!("Collect failed: {e}")),
            });
            ctx.request_repaint();
        });
    }

    /// Render the timeline through every platform preset into `dir`.
    fn start_export_all(&mut self, dir: PathBuf, ctx: &egui::Context) {
        if self.timeline.clips.is_empty() {
            self.status = "Nothing on the timeline to export.".into();
            return;
        }
        let (seq_refs, _) = build_playback_sequence(
            &self.timeline.clips,
            &self.packet_clips,
            self.timeline.total_frame_count() as i64,
            self.timeline.playhead,
        );
        if seq_refs.is_empty() {
            self.status = "Nothing on the timeline to export.".into();
            return;
        }
        let render_packets: Vec<OwnedPacket> = seq_refs
            .iter()
            .enumerate()
            .map(|(i, (pkt, _))| OwnedPacket {
                data: pkt.data.clone(),
                pts: i as i64,
                dts: i as i64,
                duration: 1,
                is_key: pkt.is_key,
            })
            .collect();
        let leading_clip_idx = seq_refs[0].1;

        self.proj_busy = true;
        self.status = "Exporting all platform presets…".into();
        let fps = self.render_fps;
        let codec_params = self.packet_clips[leading_clip_idx].codec_parameters.clone();
        let time_base = ffmpeg_next::Rational(1, fps as i32);
        let total_frames = self.timeline.total_frame_count();
        let audio_sources = self.audio_clips.clone();
        let audio_timeline: Vec<AudioTimelineClip> = self.timeline.audio_clips.clone();
        let tx = self.proj_msg_tx.clone();
        let ctx = ctx.clone();

        std::thread::spawn(move || {
            let result = (|| -> Result<PathBuf, String> {
                let temp_dir = tempfile::tempdir().map_err(|e| format!("Temp dir: {e}"))?;
                let video_temp = temp_dir.path().join("video.mp4");
                export_packets(&render_packets, &video_temp, &codec_params, time_base)
                    .map_err(|e| format!("Export: {e}"))?;

                let audio_temp_path: Option<String> = if !audio_timeline.is_empty() {
                    let audio_temp = temp_dir.path().join("audio.wav");
                    crate::audio::render_audio_mix(
                        &audio_sources,
                        &audio_timeline,
                        total_frames,
                        fps,
                        &audio_temp,
                    )
                    .map_err(|e| format!("Audio mix: {e}"))?;
                    Some(audio_temp.to_string_lossy().into_owned())
                } else {
                    None
                };

                for preset in ExportPreset::ALL_PLATFORMS {
                    let _ = tx.send(ProjectMsg::Progress(format!(
                        "Exporting {}…",
                        preset.label()
                    )));
                    ctx.request_repaint();
                    let out = dir.join(format!("mosh_{}.mp4", preset.file_tag()));
                    let args = preset.build_ffmpeg_args(
                        &video_temp.to_string_lossy(),
                        audio_temp_path.as_deref(),
                        &out.to_string_lossy(),
                        fps,
                    );
                    let output = std::process::Command::new(crate::bundled_ffmpeg())
                        .args(&args)
                        .stdin(std::process::Stdio::null())
                        .stdout(std::process::Stdio::null())
                        .stderr(std::process::Stdio::piped())
                        .output()
                        .map_err(|e| format!("ffmpeg launch: {e}"))?;
                    if !output.status.success() {
                        let stderr = String::from_utf8_lossy(&output.stderr);
                        return Err(format!("{} failed: {stderr}", preset.label()));
                    }
                }
                Ok(dir)
            })();
            let _ = tx.send(match result {
                Ok(p) => ProjectMsg::ExportedAll(p),
                Err(e) => ProjectMsg::Error(format!("Export-all failed: {e}")),
            });
            ctx.request_repaint();
        });
    }

    /// Replace all live state with a freshly loaded project.
    fn apply_loaded(&mut self, lp: LoadedProject, path: PathBuf) {
        self.packet_clips = lp.packet_clips;
        self.audio_clips = lp.audio_clips;
        self.timeline.clips = lp.video_timeline;
        self.timeline.audio_clips = lp.audio_timeline;
        self.render_fps = lp.render_fps;
        self.export_preset = lp.export_preset;
        self.timeline.zoom = lp.zoom;
        self.timeline.playhead = lp.playhead;
        self.timeline.clear_selection();

        let max_tl_id = self.timeline.clips.iter().map(|c| c.id).max().unwrap_or(0);
        self.timeline.set_next_id(max_tl_id + 1);
        self.clip_uid = self.packet_clips.iter().map(|c| c.id).max().unwrap_or(0) + 1;
        self.color_idx = self.timeline.clips.len();

        // Fresh preview worker for the loaded media.
        self.preview_req_tx = None;
        self.preview_cache = None;
        self.ensure_preview_worker();

        self.undo_stack.clear();
        self.redo_stack.clear();
        self.last_snapshot = self.snapshot();

        self.project_path = Some(path.clone());
        self.project_dirty = false;
        self.recovery_available = None;
        project::push_recent(&path);
        self.recent_projects = project::load_recent();
        self.status = format!("Opened {}", path.display());
    }

    /// Drain the project dialog + result channels.
    fn poll_project_io(&mut self, ctx: &egui::Context) {
        if let Ok(req) = self.proj_req_rx.try_recv() {
            self.handle_project_request(req, ctx);
        }
        while let Ok(msg) = self.proj_msg_rx.try_recv() {
            match msg {
                ProjectMsg::Loaded(lp, path) => {
                    self.proj_busy = false;
                    self.apply_loaded(*lp, path);
                }
                ProjectMsg::Collected(zip) => {
                    self.proj_busy = false;
                    self.status = format!("Collected shareable bundle → {}", zip.display());
                }
                ProjectMsg::ExportedAll(dir) => {
                    self.proj_busy = false;
                    self.status = format!("Exported all platform presets → {}", dir.display());
                }
                ProjectMsg::Progress(s) => self.status = s,
                ProjectMsg::Error(e) => {
                    self.proj_busy = false;
                    self.status = e;
                }
            }
        }
    }

    /// Periodically write a recovery bundle so a crash doesn't lose work.
    fn maybe_autosave(&mut self) {
        const INTERVAL: std::time::Duration = std::time::Duration::from_secs(120);
        if !self.project_dirty || self.proj_busy || self.bake_in_progress || self.is_rendering {
            return;
        }
        if self.last_autosave.elapsed() < INTERVAL {
            return;
        }
        self.last_autosave = Instant::now();
        if let Some(dir) = project::autosave_path() {
            let req = self.save_request();
            match project::save_bundle(&dir, &req) {
                Ok(()) => self.status = "Autosaved recovery snapshot.".into(),
                Err(e) => self.status = format!("Autosave failed: {e}"),
            }
        }
    }

    /// Detect a completed edit (timeline state changed while not mid-drag) and
    /// push the previous state onto the undo stack.
    fn commit_edit_if_changed(&mut self) {
        if self.timeline.is_dragging() {
            return;
        }
        let current = self.snapshot();
        if current != self.last_snapshot {
            self.undo_stack.push(std::mem::replace(&mut self.last_snapshot, current));
            self.redo_stack.clear();
            if self.undo_stack.len() > 100 {
                self.undo_stack.remove(0);
            }
            self.mark_dirty();
        }
    }

    fn undo(&mut self) {
        if let Some(prev) = self.undo_stack.pop() {
            let current = self.snapshot();
            self.redo_stack.push(current);
            self.timeline.clips = prev.clips.clone();
            self.timeline.audio_clips = prev.audio_clips.clone();
            self.last_snapshot = prev;
            self.preview_cache = None;
            self.mark_dirty();
            self.status = "Undo".into();
        } else {
            self.status = "Nothing to undo.".into();
        }
    }

    fn redo(&mut self) {
        if let Some(next) = self.redo_stack.pop() {
            self.undo_stack.push(self.snapshot());
            self.timeline.clips = next.clips.clone();
            self.timeline.audio_clips = next.audio_clips.clone();
            self.last_snapshot = next;
            self.preview_cache = None;
            self.mark_dirty();
            self.status = "Redo".into();
        } else {
            self.status = "Nothing to redo.".into();
        }
    }

    // ── File picker ───────────────────────────────────────────────────────────

    fn open_file(&self, ctx: &egui::Context) {
        let tx = self.file_tx.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            if let Some(p) = rfd::FileDialog::new()
                .add_filter("Video", &["mp4", "mov", "mkv", "avi", "webm", "m4v"])
                .add_filter("Audio", &["wav", "mp3", "aac", "flac", "m4a", "ogg"])
                .pick_file()
            {
                let _ = tx.send(p);
                ctx.request_repaint();
            }
        });
    }

    // ── Import ────────────────────────────────────────────────────────────────

    fn start_import(&mut self, path: PathBuf, ctx: &egui::Context) {
        self.status = format!("Importing {}…", path.display());
        let tx = self.import_tx.clone();
        let ctx = ctx.clone();
        let fps = self.render_fps;
        std::thread::spawn(move || {
            let name = path
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();

            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
            let is_audio = ["wav", "mp3", "aac", "flac", "m4a", "ogg"].contains(&ext.as_str());

            let result = if is_audio {
                import_audio(&path, &name, fps)
                    .map(|audio_clip| ImportResult::Audio { name, audio_clip })
                    .map_err(|e| format!("Audio import failed: {e}"))
            } else {
                import_video(&path, &name)
                    .map(|(packet_clip, _first_yuv)| {
                        // Also extract the embedded audio track, if any.
                        let audio_clip = import_audio(&path, &name, fps).ok();
                        ImportResult::Video { name, packet_clip, audio_clip }
                    })
                    .map_err(|e| format!("Video import failed: {e}"))
            };
            let _ = tx.send(result);
            ctx.request_repaint();
        });
    }

    fn finish_import(&mut self, r: ImportResult, ctx: &egui::Context) {
        match r {
            ImportResult::Video { name, packet_clip, audio_clip } => {
                let clip_idx = self.packet_clips.len();
                self.packet_clips.push(packet_clip);
                let frame_count = self.packet_clips[clip_idx].packets.len();

                self.ensure_preview_worker();

                let start_frame = self
                    .timeline
                    .clips
                    .iter()
                    .filter(|c| c.track == 1)
                    .map(|c| c.end_frame())
                    .max()
                    .unwrap_or(0);
                self.timeline.clips.push(TimelineClip {
                    id: self.clip_uid,
                    clip_idx,
                    name: name.clone(),
                    frame_count,
                    source_frame_count: frame_count,
                    start_frame,
                    source_offset: 0,
                    color: next_clip_color(self.color_idx),
                    selected: false,
                    drop_leading_keyframe: false,
                    track: 1,
                });
                self.clip_uid += 1;
                self.color_idx += 1;

                // Place the embedded audio track (if any) at the same timeline
                // position as the video clip just added.
                if let Some(ac) = audio_clip {
                    let audio_frame_count = ac.peaks.len();
                    self.audio_clips.push(ac);
                    let audio_clip_idx = self.audio_clips.len() - 1;
                    self.timeline.audio_clips.push(AudioTimelineClip {
                        audio_clip_idx,
                        start_frame,
                        frame_count: audio_frame_count,
                        source_offset: 0,
                        fade_in_frames: 0,
                        fade_out_frames: 0,
                        selected: false,
                    });
                    self.timeline.audio_clips.sort_by_key(|c| c.start_frame);
                    self.status = format!("'{}' ready — {} frames (audio extracted).", name, frame_count);
                } else {
                    self.status = format!("'{}' ready — {} frames.", name, frame_count);
                }
            }
            ImportResult::Audio { name, audio_clip } => {
                let frame_count = audio_clip.peaks.len();
                self.audio_clips.push(audio_clip);
                let clip_idx = self.audio_clips.len() - 1;

                let start_frame = self.timeline.audio_clips.iter().map(|c| c.end_frame()).max().unwrap_or(0);
                self.timeline.audio_clips.push(AudioTimelineClip {
                    audio_clip_idx: clip_idx,
                    start_frame,
                    frame_count,
                    source_offset: 0,
                    fade_in_frames: 0,
                    fade_out_frames: 0,
                    selected: false,
                });
                self.status = format!("'{}' ready — {} frames audio.", name, frame_count);
            }
        }
        self.preview_cache = None;
        ctx.request_repaint();
    }

    // ── Mosh operations ───────────────────────────────────────────────────────

    fn cross_clip_mosh(&mut self, b_idx: usize) {
        if !self.timeline.has_mosh_predecessor(b_idx) {
            self.status = "No clip covers the preceding frame — nothing to mosh against.".into();
            return;
        }

        let clip = &mut self.timeline.clips[b_idx];
        clip.drop_leading_keyframe = true;
        if clip.frame_count > 1 {
            clip.frame_count -= 1;
        }
        self.preview_cache = None;
        self.status = format!("Cross-clip mosh: dropped leading keyframe of '{}'.", clip.name);
    }

    fn remove_selected_clips(&mut self) {
        let before_v = self.timeline.clips.len();
        let before_a = self.timeline.audio_clips.len();
        self.timeline.clips.retain(|c| !c.selected);
        self.timeline.audio_clips.retain(|c| !c.selected);
        let removed_v = before_v - self.timeline.clips.len();
        let removed_a = before_a - self.timeline.audio_clips.len();
        if removed_v > 0 || removed_a > 0 {
            self.timeline.validate_mosh_state();
            self.preview_cache = None;
            self.status = format!("Removed {removed_v} video clip(s), {removed_a} audio clip(s) from timeline.");
        }
    }

    // ── Glitch dialogs ────────────────────────────────────────────────────────

    /// Confirmation modal for "New Project" when there are unsaved changes.
    fn draw_new_confirm(&mut self, ctx: &egui::Context) {
        if !self.show_new_confirm {
            return;
        }
        egui::Window::new("New Project")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label("Discard the current project and start a new one?");
                ui.label(
                    egui::RichText::new("Unsaved changes will be lost.")
                        .small()
                        .weak(),
                );
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Discard & start new").clicked() {
                        self.new_project();
                    }
                    if ui.button("Save first…").clicked() {
                        self.show_new_confirm = false;
                        self.new_after_save = true;
                        self.save_or_prompt(ctx);
                    }
                    if ui.button("Cancel").clicked() {
                        self.show_new_confirm = false;
                    }
                });
            });
    }

    fn draw_bend_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_bend_dialog {
            return;
        }
        let ctx_clone = ctx.clone();
        let mut open = self.show_bend_dialog;
        let busy = self.bake_in_progress;
        egui::Window::new("🌀 Data Bend")
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.label("Duration:");
                ui.horizontal(|ui| {
                    ui.radio_value(&mut self.bend_duration, 0, "Current frame");
                    ui.radio_value(&mut self.bend_duration, 1, "±5f");
                    ui.radio_value(&mut self.bend_duration, 2, "±15f");
                    ui.radio_value(&mut self.bend_duration, 3, "Whole clip");
                });
                ui.add_space(8.0);

                ui.label("Mode:");
                let modes = [
                    "Reverse scanlines",
                    "Echo",
                    "Bitcrush",
                    "Byte swap",
                    "XOR mask",
                    "Noise",
                ];
                egui::ComboBox::from_label("")
                    .selected_text(modes[self.bend_mode.min(modes.len() - 1)])
                    .show_ui(ui, |ui| {
                        for (i, name) in modes.iter().enumerate() {
                            ui.selectable_value(&mut self.bend_mode, i, *name);
                        }
                    });

                match self.bend_mode {
                    1 => {
                        ui.horizontal(|ui| {
                            ui.label("Delay:");
                            ui.add(egui::DragValue::new(&mut self.bend_echo_delay).range(1..=64));
                        });
                        ui.horizontal(|ui| {
                            ui.label("Mix:");
                            ui.add(egui::Slider::new(&mut self.bend_echo_mix, 0.0..=1.0));
                        });
                    }
                    2 => {
                        ui.horizontal(|ui| {
                            ui.label("Bits:");
                            ui.add(egui::DragValue::new(&mut self.bend_bitcrush_bits).range(1..=8));
                        });
                    }
                    3 => {
                        ui.horizontal(|ui| {
                            ui.label("Stride:");
                            ui.add(egui::DragValue::new(&mut self.bend_byteswap_stride).range(2..=64));
                        });
                    }
                    4 => {
                        ui.horizontal(|ui| {
                            ui.label("Mask:");
                            ui.add(egui::DragValue::new(&mut self.bend_xor_mask).range(0..=255));
                        });
                    }
                    5 => {
                        ui.horizontal(|ui| {
                            ui.label("Amount:");
                            ui.add(egui::DragValue::new(&mut self.bend_noise_amount).range(1..=128));
                        });
                    }
                    _ => {}
                }

                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    let apply = ui.add_enabled(!busy, egui::Button::new("Apply"));
                    if apply.clicked() {
                        self.apply_bake(Effect::Bend(self.bend_mode_from_state()), &ctx_clone);
                        self.show_bend_dialog = false;
                    }
                    if ui.button("Cancel").clicked() {
                        self.show_bend_dialog = false;
                    }
                });
            });
        self.show_bend_dialog = open;
    }

    fn bend_mode_from_state(&self) -> BendMode {
        match self.bend_mode {
            0 => BendMode::ReverseScanlines,
            1 => BendMode::Echo { delay: self.bend_echo_delay, mix: self.bend_echo_mix },
            2 => BendMode::Bitcrush { bits: self.bend_bitcrush_bits },
            3 => BendMode::ByteSwap { stride: self.bend_byteswap_stride },
            4 => BendMode::Xor { mask: self.bend_xor_mask },
            5 => BendMode::Noise {
                amount: self.bend_noise_amount,
                seed: rand::random::<u64>(),
            },
            _ => BendMode::ReverseScanlines,
        }
    }

    /// Resolve the selected clip's native dimensions for compress-dialog bounds.
    fn selected_clip_dims(&self) -> (u32, u32) {
        self.timeline
            .selected_video_idx()
            .and_then(|i| self.packet_clips.get(self.timeline.clips[i].clip_idx))
            .map(|c| (c.width.max(1), c.height.max(1)))
            .unwrap_or((1920, 1080))
    }

    fn draw_compress_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_compress_dialog {
            return;
        }
        let ctx_clone = ctx.clone();
        let mut open = self.show_compress_dialog;
        let busy = self.bake_in_progress;
        let (fw, fh) = self.selected_clip_dims();
        let current_clip_id = self
            .timeline
            .selected_video_idx()
            .map(|i| self.timeline.clips[i].id);
        if self.compress_dialog_clip_id != current_clip_id {
            self.compress_dialog_clip_id = current_clip_id;
            self.compress_x = 0;
            self.compress_y = 0;
            self.compress_w = fw;
            self.compress_h = fh;
        }
        // Keep prior values inside the new clip's bounds.
        self.compress_x = self.compress_x.min(fw.saturating_sub(1));
        self.compress_y = self.compress_y.min(fh.saturating_sub(1));
        self.compress_w = self.compress_w.clamp(1, fw - self.compress_x);
        self.compress_h = self.compress_h.clamp(1, fh - self.compress_y);

        egui::Window::new("🗜 Compress Region")
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.label("Duration:");
                ui.horizontal(|ui| {
                    ui.radio_value(&mut self.compress_duration, 0, "Current frame");
                    ui.radio_value(&mut self.compress_duration, 1, "±5f");
                    ui.radio_value(&mut self.compress_duration, 2, "±15f");
                    ui.radio_value(&mut self.compress_duration, 3, "Whole clip");
                });
                ui.add_space(8.0);

                ui.label(format!("Region (x, y, w, h) — frame is {fw}×{fh}:"));
                ui.horizontal(|ui| {
                    ui.add(egui::DragValue::new(&mut self.compress_x).range(0..=fw.saturating_sub(1)));
                    ui.add(egui::DragValue::new(&mut self.compress_y).range(0..=fh.saturating_sub(1)));
                    ui.add(egui::DragValue::new(&mut self.compress_w).range(1..=fw - self.compress_x));
                    ui.add(egui::DragValue::new(&mut self.compress_h).range(1..=fh - self.compress_y));
                });
                ui.label("Quality (lower = more artifacts):");
                ui.add(egui::Slider::new(&mut self.compress_quality, 1..=100));

                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    let apply = ui.add_enabled(!busy, egui::Button::new("Apply"));
                    if apply.clicked() {
                        let region = CompressRegion {
                            x: self.compress_x,
                            y: self.compress_y,
                            w: self.compress_w,
                            h: self.compress_h,
                            quality: self.compress_quality,
                        };
                        self.apply_bake(Effect::Compress(region), &ctx_clone);
                        self.show_compress_dialog = false;
                    }
                    if ui.button("Cancel").clicked() {
                        self.show_compress_dialog = false;
                    }
                });
            });
        self.show_compress_dialog = open;
    }

    /// Map the dialog "Duration" radio index to a `(start, count)` pair in
    /// source-packet coordinates, clamped to the clip's packet range.
    fn duration_to_range(
        duration_idx: usize,
        playhead_local: usize,
        total_source: usize,
        clip_offset: usize,
        clip_count: usize,
    ) -> (usize, usize) {
        let clamped_ph = playhead_local.min(total_source.saturating_sub(1));
        match duration_idx {
            0 => (clamped_ph, 1),
            1 => {
                let s = clamped_ph.saturating_sub(5);
                (s, 11.min(total_source - s))
            }
            2 => {
                let s = clamped_ph.saturating_sub(15);
                (s, 31.min(total_source - s))
            }
            _ => (clip_offset, clip_count),
        }
    }

    fn apply_bake(&mut self, effect: Effect, ctx: &egui::Context) {
        if self.bake_in_progress {
            self.status = "A bake is already running.".into();
            return;
        }
        let Some(sel_idx) = self.timeline.selected_video_idx() else {
            self.status = "No clip selected.".into();
            return;
        };

        let tl_clip = &self.timeline.clips[sel_idx];
        let source_clip = match self.packet_clips.get(tl_clip.clip_idx) {
            Some(c) => c,
            None => {
                self.status = "Source clip not found.".into();
                return;
            }
        };

        let total_source = source_clip.packets.len();
        let playhead_local = (self.timeline.playhead - tl_clip.start_frame)
            .max(0) as usize
            + tl_clip.source_offset;

        let duration = match &effect {
            Effect::Bend(_) => self.bend_duration,
            Effect::Compress(_) => self.compress_duration,
        };

        // A timeline-ruler selection overrides the Duration radio. Map it from
        // timeline frames into source-packet coordinates against the selected
        // clip; if it doesn't overlap the clip, bail out rather than silently
        // falling back.
        let drop_skip = if tl_clip.drop_leading_keyframe { 1 } else { 0 };
        let visible_first = tl_clip.source_offset + drop_skip;
        let visible_end = visible_first + tl_clip.frame_count;

        let (start, count) = if let Some((sel_a, sel_b)) = self.timeline.selection {
            let (sel_lo, sel_hi) = if sel_b >= sel_a { (sel_a, sel_b) } else { (sel_b, sel_a) };
            let tl_start = tl_clip.start_frame;
            let tl_end = tl_start + tl_clip.frame_count as i64;
            let lo = sel_lo.max(tl_start);
            let hi = sel_hi.min(tl_end);
            if hi <= lo {
                self.status = "Selection doesn't overlap the selected clip.".into();
                return;
            }
            let local_start = (lo - tl_start) as usize;
            let local_end = (hi - tl_start) as usize;
            (
                visible_first + local_start,
                local_end.saturating_sub(local_start),
            )
        } else {
            Self::duration_to_range(
                duration,
                playhead_local,
                total_source,
                tl_clip.source_offset,
                tl_clip.frame_count,
            )
        };

        // Clamp the bake range to the visible portion of this TimelineClip.
        // Source packets played by the clip start at source_offset + drop_skip
        // (the leading-keyframe skip introduced by cross-clip mosh) and run for
        // frame_count packets. Baking outside that window would desync pre/post.
        let start_clamped = start.max(visible_first).min(visible_end);
        let count_clamped = count.min(visible_end.saturating_sub(start_clamped));

        if count_clamped == 0 {
            self.status = "Nothing to bake in the selected range.".into();
            return;
        }

        let source_name = tl_clip.name.clone();
        let source_tl_clip_id = tl_clip.id;
        let source_clip = source_clip.clone();
        let fps = self.render_fps;
        let tx = self.bake_tx.clone();
        self.bake_cancel = Arc::new(AtomicBool::new(false));
        let cancel = self.bake_cancel.clone();
        let ctx_for_thread = ctx.clone();
        let source_name_thread = source_name.clone();

        self.bake_in_progress = true;
        self.bake_progress = 0.0;
        self.status = format!("Baking {count_clamped} frame(s)…");

        std::thread::spawn(move || {
            let mut report = |p: f32| {
                let _ = tx.send(BakeMsg::Progress(p));
                ctx_for_thread.request_repaint();
            };
            let result = bake_segment(
                &source_clip,
                start_clamped,
                count_clamped,
                effect,
                fps,
                &mut report,
                &cancel,
            );
            let msg = match result {
                Ok(clip) => Ok(BakeDone {
                    clip,
                    source_name: source_name_thread,
                    source_tl_clip_id,
                    source_start: start_clamped,
                }),
                Err(e) => Err(format!("{e}")),
            };
            let _ = tx.send(BakeMsg::Done(msg));
            ctx_for_thread.request_repaint();
        });
    }

    fn cancel_bake(&mut self) {
        if self.bake_in_progress {
            self.bake_cancel.store(true, std::sync::atomic::Ordering::Relaxed);
            self.status = "Cancelling bake…".into();
        }
    }

    fn finish_bake(&mut self, done: BakeDone) {
        let name = format!("{}_baked_{:02}", done.source_name, self.clip_uid);
        let baked_clip_idx = self.packet_clips.len();
        let baked_frame_count = done.clip.packets.len();
        self.packet_clips
            .push(PacketClip { name: name.clone(), ..done.clip });

        // Place the baked clip on V1 (top track) at the timeline frame where
        // the source region it was baked from begins. At playback the baked
        // clip overlays its V2 source; when it ends and V2 resumes mid-GOP
        // the decoder bleeds the baked clip's final state into V2's P-frames.
        let src_pos = self
            .timeline
            .clips
            .iter()
            .position(|c| c.id == done.source_tl_clip_id);

        let start_frame = match src_pos {
            Some(pos) => {
                let src = &self.timeline.clips[pos];
                let drop_skip = if src.drop_leading_keyframe { 1 } else { 0 };
                let visible_first = src.source_offset + drop_skip;
                let bake_first = done.source_start.max(visible_first);
                src.start_frame + (bake_first.saturating_sub(visible_first)) as i64
            }
            None => self.timeline.playhead.max(0),
        };

        let new_id = self.timeline.next_id();
        self.timeline.clips.push(TimelineClip {
            id: new_id,
            clip_idx: baked_clip_idx,
            name: name.clone(),
            frame_count: baked_frame_count,
            source_frame_count: baked_frame_count,
            start_frame,
            source_offset: 0,
            color: next_clip_color(self.color_idx),
            selected: false,
            drop_leading_keyframe: false,
            track: 0,
        });
        self.clip_uid += 1;
        self.color_idx += 1;
        self.timeline.clips.sort_by_key(|c| c.start_frame);
        self.timeline.validate_mosh_state();
        self.preview_cache = None;
        self.status = format!(
            "'{}' baked to V1 at frame {} ({} frames).",
            name, start_frame, baked_frame_count
        );
    }

    // ── Render ────────────────────────────────────────────────────────────────

    fn open_render_dialog(&self, ctx: &egui::Context) {
        let tx = self.render_tx.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            if let Some(p) = rfd::FileDialog::new()
                .add_filter("MP4 video", &["mp4"])
                .set_file_name("output.mp4")
                .save_file()
            {
                let _ = tx.send(p);
                ctx.request_repaint();
            }
        });
    }

    fn start_render(&mut self, output_path: PathBuf, ctx: &egui::Context) {
        if self.timeline.clips.is_empty() {
            self.status = "Nothing on the timeline to render.".into();
            return;
        }

        // Build the two-track playback sequence, then rewrite PTS/DTS
        // monotonically so the output stream is well-formed regardless of
        // which clip each packet came from.
        let (seq_refs, _) = build_playback_sequence(
            &self.timeline.clips,
            &self.packet_clips,
            self.timeline.total_frame_count() as i64,
            self.timeline.playhead,
        );
        if seq_refs.is_empty() {
            self.status = "Nothing on the timeline to render.".into();
            return;
        }
        // Normalise every packet to one frame at render_fps, regardless of
        // which clip (source vs baked) it came from. Source and baked clips
        // carry durations stamped against their own per-clip time_bases; muxing
        // them with a single shared time_base would otherwise rescale the
        // baked clip's tiny durations into near-instant playback (fast-forward
        // through baked sections). Unifying duration = 1 at time_base = 1/fps
        // keeps every packet exactly one frame long at playback time.
        let mut render_packets: Vec<crate::packet::OwnedPacket> =
            Vec::with_capacity(seq_refs.len());
        for (i, (pkt, _)) in seq_refs.iter().enumerate() {
            render_packets.push(crate::packet::OwnedPacket {
                data: pkt.data.clone(),
                pts: i as i64,
                dts: i as i64,
                duration: 1,
                is_key: pkt.is_key,
            });
        }
        // The output stream's codec parameters must match the clip that owns the
        // leading packet (its keyframe), not always pool clip 0 — clips carry
        // different SPS/PPS, so a fixed clip-0 would misdeclare the stream and
        // break decoding whenever another clip leads (e.g. after reordering).
        let leading_clip_idx = seq_refs[0].1;

        let preset = self.export_preset;
        if preset.re_encodes() {
            self.status = format!(
                "Encoding delivery master ({}) — {} packets…",
                preset.label(),
                render_packets.len()
            );
        } else {
            self.status = format!("Rendering {} video packets…", render_packets.len());
        }
        self.is_rendering = true;

        let fps = self.render_fps;
        let tx = self.render_result_tx.clone();
        let ctx = ctx.clone();
        let codec_params = self.packet_clips[leading_clip_idx].codec_parameters.clone();
        let time_base = ffmpeg_next::Rational(1, fps as i32);
        let total_frames = self.timeline.total_frame_count();
        let audio_sources = self.audio_clips.clone();
        let audio_timeline: Vec<AudioTimelineClip> = self.timeline.audio_clips.clone();

        std::thread::spawn(move || {
            let temp_dir = tempfile::tempdir().map_err(|e| format!("Temp dir error: {e}"));
            let temp_dir = match temp_dir {
                Ok(d) => d,
                Err(e) => {
                    let _ = tx.send(Err(e));
                    ctx.request_repaint();
                    return;
                }
            };
            let video_temp = temp_dir.path().join("video.mp4");

            let video_result = export_packets(&render_packets, &video_temp, &codec_params, time_base);
            if let Err(e) = video_result {
                let _ = tx.send(Err(format!("Render error: {e}")));
                ctx.request_repaint();
                return;
            }
            let video_size = std::fs::metadata(&video_temp).map(|m| m.len()).unwrap_or(0);
            if video_size == 0 {
                let _ = tx.send(Err("Video temp file is empty after export.".into()));
                ctx.request_repaint();
                return;
            }

            let audio_temp_path: Option<String> = if !audio_timeline.is_empty() {
                let audio_temp = temp_dir.path().join("audio.wav");
                if let Err(e) = crate::audio::render_audio_mix(
                    &audio_sources,
                    &audio_timeline,
                    total_frames,
                    fps,
                    &audio_temp,
                ) {
                    let _ = tx.send(Err(format!("Audio mix error: {e}")));
                    ctx.request_repaint();
                    return;
                }
                Some(audio_temp.to_string_lossy().into_owned())
            } else {
                None
            };

            let ffmpeg_args = preset.build_ffmpeg_args(
                &video_temp.to_string_lossy(),
                audio_temp_path.as_deref(),
                &output_path.to_string_lossy(),
                fps,
            );

            let ffmpeg_output = std::process::Command::new(crate::bundled_ffmpeg())
                .args(&ffmpeg_args)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .output();

            let result = match ffmpeg_output {
                Ok(out) if out.status.success() => {
                    Ok(format!("Rendered {} packets → {}", render_packets.len(), output_path.display()))
                }
                Ok(out) => {
                    let stderr = String::from_utf8_lossy(&out.stderr);
                    Err(format!("ffmpeg muxing failed ({}): {stderr}", out.status))
                }
                Err(e) => Err(format!("ffmpeg launch failed: {e}")),
            };
            let _ = tx.send(result);
            ctx.request_repaint();
        });
    }

    // ── Preview ───────────────────────────────────────────────────────────────

    /// Returns the cached preview frame immediately and fires off a background
    /// decode request when the playhead has moved to a new frame. The UI stays
    /// responsive because we never block here — the stale cache is shown for at
    /// most one repaint cycle (≈16 ms) until the worker delivers the new frame.
    fn current_preview_yuv(&mut self) -> Option<Arc<Yuv420>> {
        let (sequence, ph_idx) = build_playback_sequence(
            &self.timeline.clips,
            &self.packet_clips,
            self.timeline.total_frame_count() as i64,
            self.timeline.playhead,
        );
        if sequence.is_empty() {
            return None;
        }
        let target = ph_idx.unwrap_or(sequence.len().saturating_sub(1));

        // Cache hit — return immediately without any decode work.
        if let Some((cached_frame, ref yuv)) = self.preview_cache {
            if cached_frame == target {
                return Some(yuv.clone());
            }
        }

        // Cache miss — ask the worker thread to decode this frame. The decoder
        // is seeded from the clip that owns the slice's leading keyframe, so
        // clips with different SPS/PPS decode correctly in any order.
        if let Some(req_tx) = &self.preview_req_tx {
            let kf_start = sequence[..=target]
                .iter()
                .rposition(|(p, _)| p.is_key)
                .unwrap_or(0);
            let owner_clip_idx = sequence[kf_start].1;
            let packets: Vec<OwnedPacket> = sequence[kf_start..=target]
                .iter()
                .map(|(p, _)| (*p).clone())
                .collect();
            let target_in_slice = target - kf_start;
            if let Some(params) = self
                .packet_clips
                .get(owner_clip_idx)
                .map(|c| c.codec_parameters.clone())
            {
                if req_tx
                    .try_send((owner_clip_idx, params, packets, target_in_slice, target))
                    .is_err()
                {
                    // Worker busy; flag so update() schedules a retry repaint.
                    self.preview_dirty = true;
                }
            }
        }

        // Return stale cache while the worker decodes the new frame.
        self.preview_cache.as_ref().map(|(_, y)| y.clone())
    }

    // ── Helpers ───────────────────────────────────────────────────────────────

    fn add_video_from_pool(&mut self, pool_idx: usize, target_frame: i64, track: u8) {
        if pool_idx >= self.packet_clips.len() { return; }
        let packet_clip = &self.packet_clips[pool_idx];
        let frame_count = packet_clip.packets.len();
        let mut start_frame = target_frame.max(0);
        start_frame = self.timeline.snap_start_frame(start_frame, frame_count, None);

        self.timeline.clips.push(TimelineClip {
            id: self.clip_uid,
            clip_idx: pool_idx,
            name: packet_clip.name.clone(),
            frame_count,
            source_frame_count: frame_count,
            start_frame,
            source_offset: 0,
            color: next_clip_color(self.color_idx),
            selected: false,
            drop_leading_keyframe: false,
            track: track.min(1),
        });
        self.clip_uid += 1;
        self.color_idx += 1;
        self.preview_cache = None;
        self.timeline.clips.sort_by_key(|c| c.start_frame);
    }

    fn add_audio_from_pool(&mut self, pool_idx: usize, target_frame: i64) {
        if pool_idx >= self.audio_clips.len() { return; }
        let audio_clip = &self.audio_clips[pool_idx];
        let frame_count = audio_clip.peaks.len();
        let mut start_frame = target_frame.max(0);
        start_frame = self.timeline.snap_start_frame(start_frame, frame_count, None);

        self.timeline.audio_clips.push(AudioTimelineClip {
            audio_clip_idx: pool_idx,
            start_frame,
            frame_count,
            source_offset: 0,
            fade_in_frames: 0,
            fade_out_frames: 0,
            selected: false,
        });
        self.preview_cache = None;
        self.timeline.audio_clips.sort_by_key(|c| c.start_frame);
    }
}

// ── Two-track packet resolver ────────────────────────────────────────────────

/// Resolve one timeline frame into the packet that feeds the decoder there,
/// picking the top track (track 0) first and falling back to the bottom
/// (track 1). Returns `None` when neither track covers the frame. The second
/// tuple element is the owning pool-clip index, needed because clips carry
/// different codec parameters.
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
            let packet_clip = packet_clips.get(clip.clip_idx)?;
            return packet_clip.packets.get(src_idx).map(|p| (p, clip.clip_idx));
        }
    }
    None
}

/// Walk the timeline frame-by-frame and collect the packet sequence the
/// decoder should replay. The second component is the playhead's index inside
/// the returned sequence, or `None` if no clip covers the playhead frame.
fn build_playback_sequence<'a>(
    clips: &'a [TimelineClip],
    packet_clips: &'a [PacketClip],
    total_frames: i64,
    playhead: i64,
) -> (Vec<(&'a OwnedPacket, usize)>, Option<usize>) {
    let mut seq: Vec<(&'a OwnedPacket, usize)> = Vec::with_capacity(total_frames.max(0) as usize);
    let mut ph_idx: Option<usize> = None;
    for f in 0..total_frames {
        if let Some(entry) = resolve_packet_at(clips, packet_clips, f) {
            if f == playhead {
                ph_idx = Some(seq.len());
            }
            seq.push(entry);
        }
    }
    (seq, ph_idx)
}

// ── eframe::App ───────────────────────────────────────────────────────────────

impl eframe::App for MoshApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // ── Drain channels ────────────────────────────────────────────────────
        if let Ok(path) = self.file_rx.try_recv() {
            self.start_import(path, ctx);
        }

        match self.import_rx.try_recv() {
            Ok(Ok(r)) => self.finish_import(r, ctx),
            Ok(Err(e)) => self.status = e,
            Err(mpsc::TryRecvError::Disconnected) => {
                self.status = "Import thread crashed — check terminal for details.".into();
            }
            Err(mpsc::TryRecvError::Empty) => {}
        }

        if let Ok(path) = self.render_rx.try_recv() {
            self.start_render(path, ctx);
        }

        match self.render_result_rx.try_recv() {
            Ok(Ok(msg)) => {
                self.status = msg;
                self.is_rendering = false;
            }
            Ok(Err(e)) => {
                self.status = e;
                self.is_rendering = false;
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                self.status = "Render thread crashed — check terminal for details.".into();
                self.is_rendering = false;
            }
            Err(mpsc::TryRecvError::Empty) => {}
        }

        if self.is_rendering {
            ctx.request_repaint_after(std::time::Duration::from_millis(500));
        }

        // Project save/load/collect/export-all dialogs and results.
        self.poll_project_io(ctx);
        if self.proj_busy {
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
        }

        // Drain bake messages (progress + completion).
        loop {
            match self.bake_rx.try_recv() {
                Ok(BakeMsg::Progress(p)) => {
                    self.bake_progress = p;
                }
                Ok(BakeMsg::Done(Ok(done))) => {
                    self.bake_in_progress = false;
                    self.bake_progress = 1.0;
                    self.finish_bake(done);
                }
                Ok(BakeMsg::Done(Err(e))) => {
                    self.bake_in_progress = false;
                    self.bake_progress = 0.0;
                    self.status = format!("Bake failed: {e}");
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    if self.bake_in_progress {
                        self.bake_in_progress = false;
                        self.status = "Bake thread crashed — check terminal for details.".into();
                    }
                    break;
                }
                Err(mpsc::TryRecvError::Empty) => break,
            }
        }
        if self.bake_in_progress {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }

        // ── Preview results ───────────────────────────────────────────────────
        while let Ok((abs_target, yuv)) = self.preview_res_rx.try_recv() {
            self.preview_cache = Some((abs_target, yuv));
            ctx.request_repaint();
        }
        if self.preview_dirty {
            self.preview_dirty = false;
            ctx.request_repaint_after(std::time::Duration::from_millis(8));
        }

        // ── Keyboard shortcuts ────────────────────────────────────────────────
        // Collect intents inside the input closure, then act (acting needs `ctx`,
        // which `ctx.input` has borrowed).
        let (do_delete, do_new, do_save, do_open, do_undo, do_redo) = ctx.input(|i| {
            let cmd = i.modifiers.command;
            let z = i.key_pressed(egui::Key::Z);
            (
                i.key_pressed(egui::Key::Delete),
                cmd && i.key_pressed(egui::Key::N),
                cmd && i.key_pressed(egui::Key::S),
                cmd && i.key_pressed(egui::Key::O),
                cmd && z && !i.modifiers.shift,
                cmd && ((z && i.modifiers.shift) || i.key_pressed(egui::Key::Y)),
            )
        });
        if do_delete {
            self.remove_selected_clips();
        }
        if do_new {
            self.request_new_project();
        }
        if do_undo {
            self.undo();
        }
        if do_redo {
            self.redo();
        }
        if do_save {
            self.save_or_prompt(ctx);
        }
        if do_open {
            self.prompt_open(ctx);
        }

        // ── Top bar ───────────────────────────────────────────────────────────
        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.menu_button("📁 Project", |ui| {
                    if ui.button("New         Ctrl+N").clicked() {
                        self.request_new_project();
                        ui.close_menu();
                    }
                    ui.separator();
                    if ui.button("Save        Ctrl+S").clicked() {
                        self.save_or_prompt(ctx);
                        ui.close_menu();
                    }
                    if ui.button("Save As…").clicked() {
                        self.prompt_save_as(ctx);
                        ui.close_menu();
                    }
                    if ui.button("Open…       Ctrl+O").clicked() {
                        self.prompt_open(ctx);
                        ui.close_menu();
                    }
                    ui.separator();
                    ui.add_enabled_ui(!self.recent_projects.is_empty(), |ui| {
                        ui.menu_button("Recent projects", |ui| {
                            let recent = self.recent_projects.clone();
                            for path in &recent {
                                let label = path
                                    .file_name()
                                    .map(|n| n.to_string_lossy().into_owned())
                                    .unwrap_or_else(|| path.to_string_lossy().into_owned());
                                if ui.button(label).on_hover_text(path.to_string_lossy()).clicked() {
                                    self.start_open(path.clone(), ctx);
                                    ui.close_menu();
                                }
                            }
                        });
                    });
                    ui.separator();
                    if ui.button("Collect files to share (.zip)…").clicked() {
                        self.prompt_collect(ctx);
                        ui.close_menu();
                    }
                    let can_export = !self.timeline.clips.is_empty();
                    if ui
                        .add_enabled(can_export, egui::Button::new("Export for all platforms…"))
                        .clicked()
                    {
                        self.prompt_export_all(ctx);
                        ui.close_menu();
                    }
                });

                ui.separator();
                if ui.button("➕ Import clip").clicked() {
                    self.open_file(ctx);
                }

                ui.separator();
                if ui
                    .add_enabled(!self.undo_stack.is_empty(), egui::Button::new("↶ Undo"))
                    .on_hover_text("Ctrl+Z")
                    .clicked()
                {
                    self.undo();
                }
                if ui
                    .add_enabled(!self.redo_stack.is_empty(), egui::Button::new("↷ Redo"))
                    .on_hover_text("Ctrl+Shift+Z")
                    .clicked()
                {
                    self.redo();
                }

                if self.is_rendering || self.proj_busy {
                    ui.separator();
                    ui.spinner();
                }
                ui.separator();
                ui.label(format!("[{}]", self.project_title()));
                ui.label(&self.status);
            });

            // Crash-recovery offer (only until dismissed/used).
            if let Some(rec) = self.recovery_available.clone() {
                ui.horizontal(|ui| {
                    ui.colored_label(
                        egui::Color32::from_rgb(230, 180, 60),
                        "⚠ An autosaved recovery snapshot was found.",
                    );
                    if ui.button("Recover").clicked() {
                        self.recovery_available = None;
                        self.start_open(rec.clone(), ctx);
                    }
                    if ui.button("Dismiss").clicked() {
                        self.recovery_available = None;
                    }
                });
            }
        });

        // ── Pool sidebar ──────────────────────────────────────────────────────
        egui::SidePanel::left("pool").min_width(160.0).show(ctx, |ui| {
            ui.heading("Clip Pool");
            ui.separator();

            if !self.packet_clips.is_empty() {
                ui.label("Video");
                for (idx, clip) in self.packet_clips.iter().enumerate() {
                    let label = format!("{} ({}f)", clip.name, clip.packets.len());
                    let response = ui.dnd_drag_source(egui::Id::new(("pool_v", idx)), PoolDragPayload::Video(idx), |ui| {
                        ui.horizontal(|ui| {
                            ui.label("🎬");
                            ui.label(&label);
                        }).response
                    });
                    if response.inner.hovered() {
                        ui.output_mut(|o| o.cursor_icon = CursorIcon::Grab);
                    }
                }
                ui.add_space(8.0);
            }

            if !self.audio_clips.is_empty() {
                ui.label("Audio");
                for (idx, clip) in self.audio_clips.iter().enumerate() {
                    let label = format!("{} ({}f)", clip.name, clip.peaks.len());
                    let response = ui.dnd_drag_source(egui::Id::new(("pool_a", idx)), PoolDragPayload::Audio(idx), |ui| {
                        ui.horizontal(|ui| {
                            ui.label("🔊");
                            ui.label(&label);
                        }).response
                    });
                    if response.inner.hovered() {
                        ui.output_mut(|o| o.cursor_icon = CursorIcon::Grab);
                    }
                }
                ui.add_space(8.0);
            }

            if self.packet_clips.is_empty() && self.audio_clips.is_empty() {
                ui.label("Import a clip to begin.");
            }
        });

        // ── Controls sidebar ──────────────────────────────────────────────────
        egui::SidePanel::right("controls").min_width(210.0).show(ctx, |ui| {
            ui.heading("Operations");
            ui.separator();

            let sel_idx = self.timeline.selected_video_idx();

            if let Some(idx) = sel_idx {
                let name = self.timeline.clips[idx].name.clone();
                ui.label(format!("Selected: {name}"));
                ui.add_space(6.0);

                let has_prev = self.timeline.has_mosh_predecessor(idx);

                let already_moshed = self.timeline.clips[idx].drop_leading_keyframe;

                if ui
                    .add_enabled(
                        has_prev && !already_moshed,
                        egui::Button::new("⚡ Cross-clip mosh"),
                    )
                    .on_hover_text(
                        "Drop the leading keyframe of this clip so that\n\
                         its P-frames decode against the preceding clip.",
                    )
                    .clicked()
                {
                    self.cross_clip_mosh(idx);
                }

                if already_moshed {
                    ui.label("Leading keyframe dropped.");
                }

                ui.add_space(8.0);
                if ui.button("🗑 Remove from timeline").clicked() {
                    self.remove_selected_clips();
                }

                ui.add_space(12.0);
                ui.separator();
                ui.heading("Glitch");
                ui.add_space(4.0);
                let busy = self.bake_in_progress;
                if ui
                    .add_enabled(!busy, egui::Button::new("🌀 Bend at playhead"))
                    .clicked()
                {
                    self.show_bend_dialog = true;
                }
                if ui
                    .add_enabled(!busy, egui::Button::new("🗜 Compress region"))
                    .clicked()
                {
                    self.show_compress_dialog = true;
                }
                if busy {
                    ui.add_space(4.0);
                    ui.add(
                        egui::ProgressBar::new(self.bake_progress)
                            .show_percentage()
                            .animate(true),
                    );
                    if ui.button("✖ Cancel bake").clicked() {
                        self.cancel_bake();
                    }
                }
            } else {
                ui.label("(no clip selected)");
                ui.add_space(6.0);
                ui.add_enabled(false, egui::Button::new("⚡ Cross-clip mosh"));

                ui.add_space(12.0);
                ui.separator();
                ui.heading("Glitch");
                ui.add_space(4.0);
                ui.add_enabled(false, egui::Button::new("🌀 Bend at playhead"));
                ui.add_enabled(false, egui::Button::new("🗜 Compress region"));
                if self.bake_in_progress {
                    ui.add_space(4.0);
                    ui.add(
                        egui::ProgressBar::new(self.bake_progress)
                            .show_percentage()
                            .animate(true),
                    );
                    if ui.button("✖ Cancel bake").clicked() {
                        self.cancel_bake();
                    }
                }
            }

            ui.add_space(16.0);
            ui.separator();
            ui.heading("Render");
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label("FPS:");
                ui.add(egui::DragValue::new(&mut self.render_fps).range(1..=120));
            });
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label("Preset:");
                egui::ComboBox::from_id_salt("export_preset")
                    .selected_text(self.export_preset.label())
                    .show_ui(ui, |ui| {
                        for preset in ExportPreset::ALL {
                            ui.selectable_value(
                                &mut self.export_preset,
                                preset,
                                preset.label(),
                            );
                        }
                    })
                    .response
                    .on_hover_text(
                        "Re-encodes the moshed output into a clean, platform-shaped \
                         master. The glitch lives in the pixels, so it survives — but \
                         the platform now compresses from a pristine source instead of \
                         mangling the fragile mosh bitstream.",
                    );
            });
            ui.label(
                egui::RichText::new(self.export_preset.description())
                    .small()
                    .weak(),
            );
            ui.add_space(4.0);
            if self.is_rendering {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Rendering…");
                });
            } else if ui
                .add_enabled(
                    !self.timeline.clips.is_empty(),
                    egui::Button::new("🎬 Render to file…"),
                )
                .clicked()
            {
                self.open_render_dialog(ctx);
            }

            ui.add_space(16.0);
            ui.separator();
            ui.heading("Timeline");
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label("Zoom:");
                if ui.small_button("−").clicked() {
                    self.timeline.zoom = (self.timeline.zoom * 0.75).clamp(0.02, 500.0);
                }
                if ui.small_button("+").clicked() {
                    self.timeline.zoom = (self.timeline.zoom * 1.3333).clamp(0.02, 500.0);
                }
                ui.add(
                    egui::Slider::new(&mut self.timeline.zoom, 0.02..=40.0)
                        .logarithmic(true)
                        .show_value(false),
                );
            });
            if self.timeline.selection.is_some()
                && ui.small_button("Clear selection").clicked()
            {
                self.timeline.clear_selection();
            }
            ui.label("Ctrl+scroll to zoom\nScroll to pan\nDrag on ruler to select");
        });

        // ── Timeline (bottom) ─────────────────────────────────────────────────
        egui::TopBottomPanel::bottom("timeline_panel")
            .min_height(100.0)
            .show(ctx, |ui| {
                ui.add_space(4.0);
                let fps = self.render_fps;
                let tl_resp = self.timeline.show(ui, fps, &self.audio_clips);
                ui.add_space(4.0);

                if let (Some(payload), Some(drop_frame)) = (tl_resp.dropped_payload, tl_resp.drop_frame) {
                    match payload {
                        PoolDragPayload::Video(pool_idx) => {
                            let track = if tl_resp.drop_is_audio { 0 } else { tl_resp.drop_track };
                            self.add_video_from_pool(pool_idx, drop_frame, track);
                        }
                        PoolDragPayload::Audio(pool_idx) => self.add_audio_from_pool(pool_idx, drop_frame),
                    }
                }
            });

        // ── Preview (centre) ──────────────────────────────────────────────────
        egui::CentralPanel::default().show(ctx, |ui| {
            let rect = ui.available_rect_before_wrap();
            if let Some(yuv) = self.current_preview_yuv() {
                ui.painter().add(egui_wgpu::Callback::new_paint_callback(
                    rect,
                    YuvPreviewCallback { yuv },
                ));
            } else {
                ui.centered_and_justified(|ui| {
                    ui.label("Import a clip to begin.");
                });
            }
        });

        // ── Glitch dialogs ────────────────────────────────────────────────────
        self.draw_bend_dialog(ctx);
        self.draw_compress_dialog(ctx);
        self.draw_new_confirm(ctx);

        // End-of-frame bookkeeping: capture an undo point if the timeline
        // changed this frame, then run the autosave heartbeat.
        self.commit_edit_if_changed();
        self.maybe_autosave();
    }
}
