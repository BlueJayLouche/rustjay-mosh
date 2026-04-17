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
const ENCODE_SEARCH_RANGE: i16 = 16;

// ── App ───────────────────────────────────────────────────────────────────────

pub struct MoshApp {
    pool: MediaPool,
    /// Flat store of all encoded frames across all clips.
    frame_store: Vec<Frame>,
    /// Decoded-frame cache, one slot per frame_store entry.
    decode_cache: Vec<Option<Arc<Yuv420>>>,
    timeline: TimelinePanel,
    color_idx: usize,
    clip_uid: u64,

    // Channels
    file_rx: mpsc::Receiver<PathBuf>,
    file_tx: mpsc::SyncSender<PathBuf>,
    render_rx: mpsc::Receiver<PathBuf>,
    render_tx: mpsc::SyncSender<PathBuf>,

    status: String,
    render_fps: u32,
}

impl MoshApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        if let Some(ws) = cc.wgpu_render_state.as_ref() {
            let res = YuvResources::new(&ws.device, ws.target_format);
            ws.renderer.write().callback_resources.insert(res);
        }

        let (file_tx, file_rx) = mpsc::sync_channel(1);
        let (render_tx, render_rx) = mpsc::sync_channel(1);

        Self {
            pool: MediaPool::new(),
            frame_store: vec![],
            decode_cache: vec![],
            timeline: TimelinePanel::new(),
            color_idx: 0,
            clip_uid: 0,
            file_rx,
            file_tx,
            render_rx,
            render_tx,
            status: "Open a video file to begin.".into(),
            render_fps: 30,
        }
    }

    // ── File operations ───────────────────────────────────────────────────────

    fn open_file(&self, ctx: &egui::Context) {
        let tx = self.file_tx.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            if let Some(p) = rfd::FileDialog::new()
                .add_filter("Video", &["mp4", "mov", "mkv", "avi", "webm"])
                .pick_file()
            {
                let _ = tx.send(p);
                ctx.request_repaint();
            }
        });
    }

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

    fn load_file(&mut self, path: PathBuf) {
        self.status = format!("Importing {}…", path.display());
        match import_video(&path) {
            Ok((raw_frames, _w, _h)) => {
                let name = path
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned();

                // Where the new frames will start in frame_store
                let pts_offset = self.frame_store.len() as u64;

                // Collect raw YUV planes from the imported I-frames
                let raw_yuv: Vec<Yuv420> = raw_frames
                    .into_iter()
                    .filter_map(|f| f.planes)
                    .collect();

                // Encode to I/P sequence and append to frame_store
                let range = encode_clip_as_ip(
                    &raw_yuv,
                    MacroblockSize::Mb16x16,
                    ENCODE_SEARCH_RANGE,
                    KEYFRAME_INTERVAL,
                    pts_offset,
                    &mut self.frame_store,
                );

                // Extend the decode cache
                self.decode_cache.resize(self.frame_store.len(), None);

                // Collect I-frame indices (local, relative to range.start)
                let i_frame_indices: Vec<usize> = (range.start..range.end)
                    .filter(|&i| {
                        matches!(self.frame_store[i].frame_type, FrameType::I)
                    })
                    .map(|i| i - range.start)
                    .collect();

                let frame_count = range.len();

                // Place the new clip after any existing clips
                let start_frame = self
                    .timeline
                    .clips
                    .iter()
                    .map(|c| c.end_frame())
                    .max()
                    .unwrap_or(0);

                let asset_id = self.pool.add_asset(
                    name.clone(),
                    MacroblockSize::Mb16x16,
                    vec![], // raw frames not kept; encoded frames live in frame_store
                );

                let color = next_clip_color(self.color_idx);
                self.color_idx += 1;

                let clip_id = self.clip_uid;
                self.clip_uid += 1;

                self.timeline.clips.push(TimelineClip {
                    id: clip_id,
                    asset_id,
                    name: name.clone(),
                    frame_count,
                    encoded_range: range,
                    i_frame_indices,
                    start_frame,
                    color,
                    selected: false,
                });

                self.status = format!(
                    "Loaded '{}' — {} frames ({} I-frames every {} frames)",
                    name,
                    frame_count,
                    frame_count.div_ceil(KEYFRAME_INTERVAL),
                    KEYFRAME_INTERVAL,
                );
            }
            Err(e) => {
                self.status = format!("Import error: {e}");
            }
        }
    }

    // ── Mosh operations ───────────────────────────────────────────────────────

    /// Rewire the first P-frame of the selected clip to reference the last
    /// frame of the previous clip (cross-clip datamosh at the boundary).
    fn cross_clip_mosh(&mut self, b_idx: usize) {
        // Sort clips so we can find the predecessor.
        let clips = &self.timeline.clips;

        let clip_b_start = clips[b_idx].start_frame;
        let clip_b_range = clips[b_idx].encoded_range.clone();

        // Previous clip: the one whose end_frame is closest to (≤) clip_b_start.
        let prev = clips
            .iter()
            .enumerate()
            .filter(|(i, c)| *i != b_idx && c.end_frame() <= clip_b_start)
            .max_by_key(|(_, c)| c.end_frame());

        let a_last_store_idx = match prev {
            Some((_, clip_a)) => clip_a.encoded_range.end.saturating_sub(1),
            None => {
                self.status = "No preceding clip to mosh with.".into();
                return;
            }
        };

        // Rewire: for every I-frame in clip B, redirect the *following* P-frame
        // to reference clip A's last frame.
        let b_i_frames = self.timeline.clips[b_idx].i_frame_indices.clone();
        let mut rewired = 0;

        for local_i in b_i_frames {
            let iframe_abs = clip_b_range.start + local_i;
            let pframe_abs = iframe_abs + 1;
            if pframe_abs >= clip_b_range.end {
                continue;
            }
            if matches!(self.frame_store[pframe_abs].frame_type, FrameType::P) {
                self.frame_store[pframe_abs].reference =
                    Some(a_last_store_idx as u32);
                rewired += 1;
            }
        }

        self.decode_cache.fill(None);
        self.status = format!("Cross-clip mosh: rewired {rewired} boundary P-frame(s).");
    }

    /// Remove interior I-frames from the selected clip by rewiring the P-frame
    /// that follows each I-frame to reference the frame *before* the I-frame.
    ///
    /// The clip's leading I-frame (local index 0) is left intact so the clip
    /// can still decode standalone; use cross-clip mosh to handle the boundary.
    fn remove_interior_iframes(&mut self, b_idx: usize) {
        let clip = &self.timeline.clips[b_idx];
        let range = clip.encoded_range.clone();

        let interior: Vec<usize> = clip
            .i_frame_indices
            .iter()
            .copied()
            .filter(|&l| l > 0) // skip the leading I-frame
            .collect();

        let mut removed = 0;
        for local_i in &interior {
            let iframe_abs = range.start + local_i;
            let pframe_abs = iframe_abs + 1;
            if pframe_abs >= range.end {
                continue;
            }
            if matches!(self.frame_store[pframe_abs].frame_type, FrameType::P) {
                // Bridge over the I-frame: P-frame -> frame before I-frame
                self.frame_store[pframe_abs].reference =
                    Some((iframe_abs.saturating_sub(1)) as u32);
                removed += 1;
            }
        }

        // Update the clip's I-frame list to reflect what's been removed
        // (keep local 0 intact; remove the rest).
        let clip = &mut self.timeline.clips[b_idx];
        clip.i_frame_indices.retain(|&l| l == 0);

        self.decode_cache.fill(None);
        self.status = format!("Removed {removed} interior I-frame(s) from '{}'.", clip.name);
    }

    // ── Render ────────────────────────────────────────────────────────────────

    fn do_render(&mut self, output_path: PathBuf) {
        let indices = self.timeline.ordered_frame_indices();
        if indices.is_empty() {
            self.status = "Nothing on the timeline to render.".into();
            return;
        }
        self.status = format!("Rendering {} frames to {}…", indices.len(), output_path.display());

        match export_video(
            &indices,
            &self.frame_store,
            &mut self.decode_cache,
            &output_path,
            self.render_fps,
        ) {
            Ok(()) => {
                self.status = format!(
                    "Rendered {} frames → {}",
                    indices.len(),
                    output_path.display()
                );
            }
            Err(e) => {
                self.status = format!("Render error: {e}");
            }
        }
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

// ── eframe::App ───────────────────────────────────────────────────────────────

impl eframe::App for MoshApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // ── Receive async messages ────────────────────────────────────────────
        if let Ok(path) = self.file_rx.try_recv() {
            self.load_file(path);
        }
        if let Ok(path) = self.render_rx.try_recv() {
            self.do_render(path);
        }

        // ── Top bar ───────────────────────────────────────────────────────────
        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui.button("➕ Import clip").clicked() {
                    self.open_file(ctx);
                }
                ui.separator();
                ui.label(&self.status);
            });
        });

        // ── Controls sidebar (right) ───────────────────────────────────────────
        egui::SidePanel::right("controls").min_width(200.0).show(ctx, |ui| {
            ui.heading("Operations");
            ui.separator();

            let sel_idx = self.timeline.selected_idx();

            if let Some(idx) = sel_idx {
                let name = self.timeline.clips[idx].name.clone();
                ui.label(format!("Selected: {name}"));
                ui.add_space(6.0);

                if ui
                    .add_enabled(
                        idx > 0
                            || self
                                .timeline
                                .clips
                                .iter()
                                .any(|c| c.end_frame() <= self.timeline.clips[idx].start_frame),
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

                if ui
                    .add_enabled(
                        !self.timeline.clips[idx].i_frame_indices.is_empty(),
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
            if ui
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
            ui.label("Ctrl+scroll to zoom, scroll to pan.");
        });

        // ── Timeline (bottom) ─────────────────────────────────────────────────
        egui::TopBottomPanel::bottom("timeline_panel")
            .min_height(100.0)
            .show(ctx, |ui| {
                ui.add_space(4.0);
                let tl_resp = self.timeline.show(ui);
                let _ = tl_resp; // playhead / selection already updated in-place
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
