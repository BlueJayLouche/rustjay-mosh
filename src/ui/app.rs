use std::path::PathBuf;
use std::sync::{mpsc, Arc};

use eframe::egui;
use eframe::egui_wgpu;

use crate::codec::encoder::encode_clip_as_ip;
use crate::codec::ir::{Frame, FrameType, MacroblockSize, Yuv420};
use crate::importer::import_video;
use crate::pool::MediaPool;
use crate::render::{decode_cached, export_video};
use crate::ui::preview::{YuvPreviewCallback, YuvResources};
use crate::ui::timeline_panel::{next_clip_color, TimelineClip, TimelinePanel};

// ── Constants ─────────────────────────────────────────────────────────────────

const KEYFRAME_INTERVAL: usize = 30;
const ENCODE_SEARCH_RANGE: i16 = 8;

// ── Background messages ────────────────────────────────────────────────────────

/// Phase 1 (fast): raw decoded I-frames arrive so the clip can appear immediately.
struct Phase1Result {
    name: String,
    raw_yuv: Vec<Yuv420>,
    store_offset: usize, // reserved slot in frame_store
}

/// Phase 2 (slow): fully encoded I/P frames replace the placeholders.
struct Phase2Result {
    clip_idx: usize,
    encoded: Vec<Frame>,
    i_frame_indices: Vec<usize>,
}

/// Render thread result: success message or error string.
type RenderResult = Result<String, String>;

// ── App ───────────────────────────────────────────────────────────────────────

pub struct MoshApp {
    pool: MediaPool,
    frame_store: Vec<Frame>,
    decode_cache: Vec<Option<Arc<Yuv420>>>,
    timeline: TimelinePanel,
    color_idx: usize,
    clip_uid: u64,

    file_rx: mpsc::Receiver<PathBuf>,
    file_tx: mpsc::SyncSender<PathBuf>,

    /// Phase 1: raw frames decoded by ffmpeg.
    p1_rx: mpsc::Receiver<Result<Phase1Result, String>>,
    p1_tx: mpsc::SyncSender<Result<Phase1Result, String>>,

    /// Phase 2: encoded I/P frames ready to splice in.
    p2_rx: mpsc::Receiver<Result<Phase2Result, String>>,
    p2_tx: mpsc::SyncSender<Result<Phase2Result, String>>,

    render_rx: mpsc::Receiver<PathBuf>,
    render_tx: mpsc::SyncSender<PathBuf>,
    render_result_rx: mpsc::Receiver<RenderResult>,
    render_result_tx: mpsc::SyncSender<RenderResult>,

    /// Clips currently being P-frame encoded (show badge on timeline).
    encoding_clips: Vec<usize>,
    is_rendering: bool,
    status: String,
    render_fps: u32,
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
        let (p1_tx, p1_rx) = mpsc::sync_channel(1);
        let (p2_tx, p2_rx) = mpsc::sync_channel(4);
        let (render_tx, render_rx) = mpsc::sync_channel(1);
        let (render_result_tx, render_result_rx) = mpsc::sync_channel(1);
        Self {
            pool: MediaPool::new(),
            frame_store: vec![],
            decode_cache: vec![],
            timeline: TimelinePanel::new(),
            color_idx: 0,
            clip_uid: 0,
            file_rx,
            file_tx,
            p1_rx,
            p1_tx,
            p2_rx,
            p2_tx,
            render_rx,
            render_tx,
            render_result_rx,
            render_result_tx,
            encoding_clips: vec![],
            is_rendering: false,
            status: "Open a video file to begin.".into(),
            render_fps: 30,
        }
    }

    // ── File picker ───────────────────────────────────────────────────────────

    fn open_file(&self, ctx: &egui::Context) {
        let tx = self.file_tx.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            if let Some(p) = rfd::FileDialog::new()
                .add_filter("Video", &["mp4", "mov", "mkv", "avi", "webm", "m4v"])
                .pick_file()
            {
                let _ = tx.send(p);
                ctx.request_repaint();
            }
        });
    }

    // ── Phase 1: decode with ffmpeg ───────────────────────────────────────────

    fn start_phase1(&mut self, path: PathBuf, ctx: &egui::Context) {
        self.status = format!("Decoding {}…", path.display());
        let store_offset = self.frame_store.len();
        let tx = self.p1_tx.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = run_phase1(path, store_offset);
            let _ = tx.send(result);
            ctx.request_repaint();
        });
    }

    fn finish_phase1(&mut self, r: Phase1Result, ctx: &egui::Context) {
        let frame_count = r.raw_yuv.len();
        let store_offset = r.store_offset;

        // Immediately fill frame_store with I-frames so preview works.
        for yuv in &r.raw_yuv {
            self.frame_store.push(crate::codec::encoder::encode_iframe(
                yuv.clone(),
                self.frame_store.len() as u64,
                MacroblockSize::Mb16x16,
            ));
        }
        self.decode_cache.resize(self.frame_store.len(), None);

        let start_frame = self
            .timeline
            .clips
            .iter()
            .map(|c| c.end_frame())
            .max()
            .unwrap_or(0);

        let asset_id = self.pool.add_asset(
            r.name.clone(),
            MacroblockSize::Mb16x16,
            vec![],
        );

        let clip_idx = self.timeline.clips.len();
        self.timeline.clips.push(TimelineClip {
            id: self.clip_uid,
            asset_id,
            name: r.name.clone(),
            frame_count,
            encoded_range: store_offset..(store_offset + frame_count),
            i_frame_indices: vec![0], // only the first frame is a keyframe for now
            start_frame,
            color: next_clip_color(self.color_idx),
            selected: false,
        });
        self.clip_uid += 1;
        self.color_idx += 1;
        self.encoding_clips.push(clip_idx);

        self.status = format!(
            "'{}' on timeline — encoding P-frames in background…",
            r.name
        );

        // Kick off phase 2.
        let tx = self.p2_tx.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = run_phase2(r.raw_yuv, store_offset, clip_idx);
            let _ = tx.send(result);
            ctx.request_repaint();
        });
    }

    // ── Phase 2: encode I/P frames ────────────────────────────────────────────

    fn finish_phase2(&mut self, r: Phase2Result) {
        let clip = &self.timeline.clips[r.clip_idx];
        let range = clip.encoded_range.clone();

        // Replace I-frame placeholders with the properly encoded I/P frames.
        for (i, frame) in r.encoded.into_iter().enumerate() {
            self.frame_store[range.start + i] = frame;
        }
        self.decode_cache.fill(None);

        self.timeline.clips[r.clip_idx].i_frame_indices = r.i_frame_indices;
        self.encoding_clips.retain(|&i| i != r.clip_idx);

        self.status = format!(
            "'{}' fully encoded — mosh operations available.",
            self.timeline.clips[r.clip_idx].name
        );
    }

    // ── Mosh operations ───────────────────────────────────────────────────────

    fn cross_clip_mosh(&mut self, b_idx: usize) {
        let clips = &self.timeline.clips;
        let clip_b_start = clips[b_idx].start_frame;
        let clip_b_range = clips[b_idx].encoded_range.clone();

        let a_last = match clips
            .iter()
            .enumerate()
            .filter(|(i, c)| *i != b_idx && c.end_frame() <= clip_b_start)
            .max_by_key(|(_, c)| c.end_frame())
        {
            Some((_, a)) => a.encoded_range.end.saturating_sub(1),
            None => {
                self.status = "No preceding clip to mosh with.".into();
                return;
            }
        };

        let b_iframes = self.timeline.clips[b_idx].i_frame_indices.clone();
        let mut rewired = 0;
        for local_i in b_iframes {
            let iframe_abs = clip_b_range.start + local_i;
            let pframe_abs = iframe_abs + 1;
            if pframe_abs < clip_b_range.end
                && matches!(self.frame_store[pframe_abs].frame_type, FrameType::P)
            {
                self.frame_store[pframe_abs].reference = Some(a_last as u32);
                rewired += 1;
            }
        }
        self.decode_cache.fill(None);
        self.status = format!("Cross-clip mosh: rewired {rewired} boundary P-frame(s).");
    }

    fn remove_interior_iframes(&mut self, b_idx: usize) {
        let range = self.timeline.clips[b_idx].encoded_range.clone();
        let interior: Vec<usize> = self.timeline.clips[b_idx]
            .i_frame_indices
            .iter()
            .copied()
            .filter(|&l| l > 0)
            .collect();

        let mut removed = 0;
        for local_i in &interior {
            let iframe_abs = range.start + local_i;
            let pframe_abs = iframe_abs + 1;
            if pframe_abs < range.end
                && matches!(self.frame_store[pframe_abs].frame_type, FrameType::P)
            {
                self.frame_store[pframe_abs].reference =
                    Some((iframe_abs.saturating_sub(1)) as u32);
                removed += 1;
            }
        }
        self.timeline.clips[b_idx].i_frame_indices.retain(|&l| l == 0);
        self.decode_cache.fill(None);
        self.status = format!(
            "Removed {removed} interior I-frame(s) from '{}'.",
            self.timeline.clips[b_idx].name
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
        let indices = self.timeline.ordered_frame_indices();
        if indices.is_empty() {
            self.status = "Nothing on the timeline to render.".into();
            return;
        }
        self.status = format!("Rendering {} frames…", indices.len());
        self.is_rendering = true;

        // Snapshot what the thread needs — clone only the frame store.
        let frame_store = self.frame_store.clone();
        let fps = self.render_fps;
        let tx = self.render_result_tx.clone();
        let ctx = ctx.clone();

        std::thread::spawn(move || {
            let mut cache: Vec<Option<Arc<Yuv420>>> = vec![None; frame_store.len()];
            let result = export_video(&indices, &frame_store, &mut cache, &output_path, fps)
                .map(|()| format!("Rendered {} frames → {}", indices.len(), output_path.display()))
                .map_err(|e| format!("Render error: {e}"));
            let _ = tx.send(result);
            ctx.request_repaint();
        });
    }

    // ── Preview ───────────────────────────────────────────────────────────────

    fn current_preview_yuv(&mut self) -> Option<Arc<Yuv420>> {
        let idx = self.timeline.store_index_at_playhead()?;
        match decode_cached(idx, &self.frame_store, &mut self.decode_cache) {
            Ok(yuv) => Some(yuv),
            Err(e) => {
                self.status = format!("Decode error: {e}");
                None
            }
        }
    }
}

// ── Background workers ────────────────────────────────────────────────────────

fn run_phase1(path: PathBuf, store_offset: usize) -> Result<Phase1Result, String> {
    let name = path
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let (raw_frames, _w, _h) =
        import_video(&path).map_err(|e| format!("Import failed: {e}"))?;
    let raw_yuv: Vec<Yuv420> = raw_frames.into_iter().filter_map(|f| f.planes).collect();
    Ok(Phase1Result { name, raw_yuv, store_offset })
}

fn run_phase2(
    raw_yuv: Vec<Yuv420>,
    store_offset: usize,
    clip_idx: usize,
) -> Result<Phase2Result, String> {
    let mut local_store: Vec<Frame> = Vec::with_capacity(raw_yuv.len());
    encode_clip_as_ip(
        &raw_yuv,
        MacroblockSize::Mb16x16,
        ENCODE_SEARCH_RANGE,
        KEYFRAME_INTERVAL,
        store_offset as u64,
        &mut local_store,
        store_offset,
    );
    let i_frame_indices: Vec<usize> = local_store
        .iter()
        .enumerate()
        .filter(|(_, f)| matches!(f.frame_type, FrameType::I))
        .map(|(i, _)| i)
        .collect();
    Ok(Phase2Result { clip_idx, encoded: local_store, i_frame_indices })
}

// ── eframe::App ───────────────────────────────────────────────────────────────

impl eframe::App for MoshApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // ── Drain channels ────────────────────────────────────────────────────
        if let Ok(path) = self.file_rx.try_recv() {
            self.start_phase1(path, ctx);
        }

        match self.p1_rx.try_recv() {
            Ok(Ok(r)) => self.finish_phase1(r, ctx),
            Ok(Err(e)) => self.status = e,
            Err(mpsc::TryRecvError::Disconnected) => {
                self.status = "Import thread crashed — check terminal for details.".into();
            }
            Err(mpsc::TryRecvError::Empty) => {}
        }

        match self.p2_rx.try_recv() {
            Ok(Ok(r)) => self.finish_phase2(r),
            Ok(Err(e)) => {
                self.status = e;
                self.encoding_clips.clear();
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                self.status = "Encoder thread crashed — check terminal for details.".into();
                self.encoding_clips.clear();
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

        // Keep repainting while encoding or rendering so the status stays live.
        if !self.encoding_clips.is_empty() || self.is_rendering {
            ctx.request_repaint_after(std::time::Duration::from_millis(500));
        }

        // ── Top bar ───────────────────────────────────────────────────────────
        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui.button("➕ Import clip").clicked() {
                    self.open_file(ctx);
                }
                if !self.encoding_clips.is_empty() {
                    ui.separator();
                    ui.spinner();
                    ui.label("Encoding…");
                }
                ui.separator();
                ui.label(&self.status);
            });
        });

        // ── Controls sidebar ──────────────────────────────────────────────────
        egui::SidePanel::right("controls").min_width(210.0).show(ctx, |ui| {
            ui.heading("Operations");
            ui.separator();

            let sel_idx = self.timeline.selected_idx();

            if let Some(idx) = sel_idx {
                let name = self.timeline.clips[idx].name.clone();
                let encoding = self.encoding_clips.contains(&idx);

                ui.label(format!("Selected: {name}"));
                if encoding {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Encoding P-frames…");
                    });
                }
                ui.add_space(6.0);

                let has_prev = {
                    let start = self.timeline.clips[idx].start_frame;
                    self.timeline
                        .clips
                        .iter()
                        .enumerate()
                        .any(|(i, c)| i != idx && c.end_frame() <= start)
                };

                if ui
                    .add_enabled(
                        !encoding && has_prev,
                        egui::Button::new("⚡ Cross-clip mosh"),
                    )
                    .on_hover_text(
                        "Rewire the first P-frame of this clip to reference\n\
                         the last frame of the preceding clip.",
                    )
                    .clicked()
                {
                    self.cross_clip_mosh(idx);
                }

                ui.add_space(4.0);

                let has_interior = self.timeline.clips[idx]
                    .i_frame_indices
                    .iter()
                    .any(|&l| l > 0);

                if ui
                    .add_enabled(
                        !encoding && has_interior,
                        egui::Button::new("🗑 Remove interior I-frames"),
                    )
                    .on_hover_text(
                        "Bridge over interior keyframes so motion flows\n\
                         continuously without resets.",
                    )
                    .clicked()
                {
                    self.remove_interior_iframes(idx);
                }
            } else {
                ui.label("(no clip selected)");
                ui.add_space(6.0);
                ui.add_enabled(false, egui::Button::new("⚡ Cross-clip mosh"));
                ui.add_space(4.0);
                ui.add_enabled(false, egui::Button::new("🗑 Remove interior I-frames"));
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
                ui.add(
                    egui::Slider::new(&mut self.timeline.zoom, 0.5..=40.0)
                        .show_value(false),
                );
            });
            ui.label("Ctrl+scroll to zoom\nScroll to pan");
        });

        // ── Timeline (bottom) ─────────────────────────────────────────────────
        egui::TopBottomPanel::bottom("timeline_panel")
            .min_height(100.0)
            .show(ctx, |ui| {
                ui.add_space(4.0);
                self.timeline.show(ui);
                ui.add_space(4.0);
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
    }
}
