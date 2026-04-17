use std::path::PathBuf;
use std::sync::{mpsc, Arc};

use eframe::egui::{self, CursorIcon};
use eframe::egui_wgpu;

use crate::codec::ir::Yuv420;
use crate::importer::import_video;
use crate::packet::{build_sequence, ClipSpan, PacketClip};
use crate::preview::decoder::PacketDecoder;
use crate::render::muxer::export_packets;
use crate::ui::preview::{YuvPreviewCallback, YuvResources};
use crate::ui::timeline_panel::{next_clip_color, TimelineClip, TimelinePanel};

// ── Background messages ────────────────────────────────────────────────────────

struct ImportResult {
    name: String,
    packet_clip: PacketClip,
}

type RenderResult = Result<String, String>;

// ── App ───────────────────────────────────────────────────────────────────────

pub struct MoshApp {
    packet_clips: Vec<PacketClip>,
    packet_decoder: Option<PacketDecoder>,
    /// Cache of the last decoded (playhead_frame, Yuv) to avoid re-decoding
    /// when the playhead is stationary.
    preview_cache: Option<(usize, Arc<Yuv420>)>,

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
        Self {
            packet_clips: vec![],
            packet_decoder: None,
            preview_cache: None,
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

    // ── Import ────────────────────────────────────────────────────────────────

    fn start_import(&mut self, path: PathBuf, ctx: &egui::Context) {
        self.status = format!("Importing {}…", path.display());
        let tx = self.import_tx.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let name = path
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            let result = import_video(&path, &name)
                .map(|(packet_clip, _first_yuv)| ImportResult {
                    name,
                    packet_clip,
                })
                .map_err(|e| format!("Import failed: {e}"));
            let _ = tx.send(result);
            ctx.request_repaint();
        });
    }

    fn finish_import(&mut self, r: ImportResult, ctx: &egui::Context) {
        let clip_idx = self.packet_clips.len();
        self.packet_clips.push(r.packet_clip);
        let packet_clip = &self.packet_clips[clip_idx];
        let frame_count = packet_clip.packets.len();

        // Initialize decoder on first import.
        if self.packet_decoder.is_none() {
            match PacketDecoder::new(&packet_clip.codec_parameters) {
                Ok(dec) => self.packet_decoder = Some(dec),
                Err(e) => {
                    self.status = format!("Decoder init failed: {e}");
                    return;
                }
            }
        }

        let start_frame = self
            .timeline
            .clips
            .iter()
            .map(|c| c.end_frame())
            .max()
            .unwrap_or(0);

        self.timeline.clips.push(TimelineClip {
            id: self.clip_uid,
            clip_idx,
            name: r.name.clone(),
            frame_count,
            source_frame_count: frame_count,
            start_frame,
            source_offset: 0,
            color: next_clip_color(self.color_idx),
            selected: false,
            drop_leading_keyframe: false,
        });
        self.clip_uid += 1;
        self.color_idx += 1;

        self.status = format!("'{}' ready — {} frames.", r.name, frame_count);
        self.preview_cache = None;
        ctx.request_repaint();
    }

    // ── Mosh operations ───────────────────────────────────────────────────────

    fn cross_clip_mosh(&mut self, b_idx: usize) {
        let clips = &self.timeline.clips;
        let clip_b_start = clips[b_idx].start_frame;

        let has_prev = clips
            .iter()
            .enumerate()
            .any(|(i, c)| i != b_idx && c.end_frame() <= clip_b_start);

        if !has_prev {
            self.status = "No preceding clip to mosh with.".into();
            return;
        }

        let clip = &mut self.timeline.clips[b_idx];
        clip.drop_leading_keyframe = true;
        // Shrinking by 1 frame keeps the timeline duration in sync with the
        // actual packet count after the leading keyframe is dropped.
        if clip.frame_count > 1 {
            clip.frame_count -= 1;
        }
        self.preview_cache = None;
        self.status = format!(
            "Cross-clip mosh: dropped leading keyframe of '{}'.",
            clip.name
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
        let sorted = self.timeline.sorted_clips();
        if sorted.is_empty() {
            self.status = "Nothing on the timeline to render.".into();
            return;
        }

        // Build a cloneable packet list for the render thread.
        let mut render_packets: Vec<crate::packet::OwnedPacket> = Vec::new();
        let mut pts_offset = 0i64;
        for (i, clip) in sorted.iter().enumerate() {
            let prev_ends = if i == 0 {
                None
            } else {
                Some(sorted[i - 1].end_frame())
            };
            let can_drop = prev_ends.map_or(false, |end| end == clip.start_frame);
            let drop = can_drop && clip.drop_leading_keyframe;
            let packet_clip = &self.packet_clips[clip.clip_idx];
            let start = clip.source_offset + if drop { 1 } else { 0 };
            let packets_iter = packet_clip.packets.iter().skip(start).take(clip.frame_count);
            if let Some(first_pkt) = packets_iter.clone().next() {
                let first = first_pkt.pts;
                for pkt in packets_iter {
                    let mut adjusted = pkt.clone();
                    adjusted.pts = pkt.pts - first + pts_offset;
                    adjusted.dts = pkt.dts - first + pts_offset;
                    render_packets.push(adjusted);
                }
                if let Some(last) = packet_clip.packets.iter().skip(start).take(clip.frame_count).last() {
                    pts_offset += last.pts + last.duration - first;
                }
            }
        }

        if render_packets.is_empty() {
            self.status = "Nothing on the timeline to render.".into();
            return;
        }

        self.status = format!("Rendering {} packets…", render_packets.len());
        self.is_rendering = true;

        let tx = self.render_result_tx.clone();
        let ctx = ctx.clone();
        let codec_params = self.packet_clips[0].codec_parameters.clone();
        let time_base = self.packet_clips[0].time_base;

        std::thread::spawn(move || {
            let result = export_packets(&render_packets, &output_path, &codec_params, time_base)
                .map(|()| format!("Rendered {} packets → {}", render_packets.len(), output_path.display()))
                .map_err(|e| format!("Render error: {e}"));
            let _ = tx.send(result);
            ctx.request_repaint();
        });
    }

    // ── Preview ───────────────────────────────────────────────────────────────

    fn current_preview_yuv(&mut self) -> Option<Arc<Yuv420>> {
        let (clip_idx, local_frame) = self.timeline.clip_at_playhead()?;
        let global_frame = self.timeline.clips[..clip_idx]
            .iter()
            .filter(|c| c.start_frame < self.timeline.playhead)
            .map(|c| c.frame_count)
            .sum::<usize>()
            + local_frame;

        if let Some((cached_frame, cached_yuv)) = &self.preview_cache {
            if *cached_frame == global_frame {
                return Some(cached_yuv.clone());
            }
        }

        // Build sequence without borrowing the whole self.
        let sorted = self.timeline.sorted_clips();
        let mut spans = Vec::with_capacity(sorted.len());
        for (i, clip) in sorted.iter().enumerate() {
            let prev_ends = if i == 0 {
                None
            } else {
                Some(sorted[i - 1].end_frame())
            };
            let can_drop = prev_ends.map_or(false, |end| end == clip.start_frame);
            let drop_leading_keyframe = can_drop && clip.drop_leading_keyframe;
            spans.push(ClipSpan {
                clip: &self.packet_clips[clip.clip_idx],
                source_offset: clip.source_offset,
                visible_count: clip.frame_count,
                drop_leading_keyframe,
            });
        }
        let sequence = build_sequence(&spans);
        if sequence.is_empty() {
            return None;
        }

        let decoder = self.packet_decoder.as_mut()?;
        match decoder.decode_up_to(&sequence, global_frame) {
            Ok(yuv) => {
                self.preview_cache = Some((global_frame, yuv.clone()));
                Some(yuv)
            }
            Err(e) => {
                self.status = format!("Decode error: {e}");
                None
            }
        }
    }

    // ── Helpers ───────────────────────────────────────────────────────────────

    fn add_timeline_clip_from_pool(&mut self, pool_idx: usize, target_frame: i64) {
        if pool_idx >= self.packet_clips.len() {
            return;
        }
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
        });
        self.clip_uid += 1;
        self.color_idx += 1;
        self.preview_cache = None;
        self.timeline.clips.sort_by_key(|c| c.start_frame);
    }
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

        // Keep repainting while rendering so the status stays live.
        if self.is_rendering {
            ctx.request_repaint_after(std::time::Duration::from_millis(500));
        }

        // ── Top bar ───────────────────────────────────────────────────────────
        egui::TopBottomPanel::top("top").show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui.button("➕ Import clip").clicked() {
                    self.open_file(ctx);
                }
                if self.is_rendering {
                    ui.separator();
                    ui.spinner();
                    ui.label("Rendering…");
                }
                ui.separator();
                ui.label(&self.status);
            });
        });

        // ── Pool sidebar ──────────────────────────────────────────────────────
        egui::SidePanel::left("pool").min_width(160.0).show(ctx, |ui| {
            ui.heading("Clip Pool");
            ui.separator();
            for (idx, clip) in self.packet_clips.iter().enumerate() {
                let label = format!("{} ({}f)", clip.name, clip.packets.len());
                let response = ui.dnd_drag_source(egui::Id::new(("pool", idx)), idx, |ui| {
                    ui.horizontal(|ui| {
                        ui.label("🎬");
                        ui.label(&label);
                    })
                    .response
                });
                if response.inner.hovered() {
                    ui.output_mut(|o| o.cursor_icon = CursorIcon::Grab);
                }
            }
            if self.packet_clips.is_empty() {
                ui.label("Import a clip to begin.");
            }
        });

        // ── Controls sidebar ──────────────────────────────────────────────────
        egui::SidePanel::right("controls").min_width(210.0).show(ctx, |ui| {
            ui.heading("Operations");
            ui.separator();

            let sel_idx = self.timeline.selected_idx();

            if let Some(idx) = sel_idx {
                let name = self.timeline.clips[idx].name.clone();
                ui.label(format!("Selected: {name}"));
                ui.add_space(6.0);

                let has_prev = {
                    let start = self.timeline.clips[idx].start_frame;
                    self.timeline
                        .clips
                        .iter()
                        .enumerate()
                        .any(|(i, c)| i != idx && c.end_frame() <= start)
                };

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
            } else {
                ui.label("(no clip selected)");
                ui.add_space(6.0);
                ui.add_enabled(false, egui::Button::new("⚡ Cross-clip mosh"));
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
                let tl_resp = self.timeline.show(ui);
                ui.add_space(4.0);

                if let (Some(pool_idx), Some(drop_frame)) =
                    (tl_resp.dropped_pool_idx, tl_resp.drop_frame)
                {
                    self.add_timeline_clip_from_pool(pool_idx, drop_frame);
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
    }
}
