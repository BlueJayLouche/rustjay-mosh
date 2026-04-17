use eframe::egui::{self, Color32, FontId, Pos2, Rect, Sense, Stroke, Vec2};
use eframe::egui::CursorIcon;

// ── Palette ──────────────────────────────────────────────────────────────────

const CLIP_COLORS: &[Color32] = &[
    Color32::from_rgb(60, 120, 200),
    Color32::from_rgb(200, 100, 50),
    Color32::from_rgb(60, 160, 80),
    Color32::from_rgb(160, 70, 160),
    Color32::from_rgb(180, 160, 40),
    Color32::from_rgb(50, 170, 170),
];

pub fn next_clip_color(idx: usize) -> Color32 {
    CLIP_COLORS[idx % CLIP_COLORS.len()]
}

// ── Data ─────────────────────────────────────────────────────────────────────

/// A clip placed on the timeline track.
#[derive(Debug, Clone)]
pub struct TimelineClip {
    pub id: u64,
    /// Index into the app's `packet_clips` vector.
    pub clip_idx: usize,
    pub name: String,
    /// Visible frame count on the timeline after trimming.
    pub frame_count: usize,
    /// Total source frame count (immutable).
    pub source_frame_count: usize,
    /// Timeline start position in *frame* units.
    pub start_frame: i64,
    /// Frames trimmed from the source head.
    pub source_offset: usize,
    pub color: Color32,
    pub selected: bool,
    /// When true, the first visible keyframe of this clip is dropped on playback,
    /// causing the decoder state to bleed in from the preceding clip.
    pub drop_leading_keyframe: bool,
}

impl TimelineClip {
    pub fn end_frame(&self) -> i64 {
        self.start_frame + self.frame_count as i64
    }

    /// Map a timeline-absolute frame to a local frame index inside this clip.
    pub fn local_frame_at(&self, timeline_frame: i64) -> Option<usize> {
        if timeline_frame < self.start_frame || timeline_frame >= self.end_frame() {
            return None;
        }
        Some((timeline_frame - self.start_frame + self.source_offset as i64) as usize)
    }
}

// ── Response ─────────────────────────────────────────────────────────────────

pub struct TimelineResponse {
    pub playhead: i64,
    pub selected_idx: Option<usize>,
    /// If a pool item was dropped onto the timeline, this is its pool index.
    pub dropped_pool_idx: Option<usize>,
    /// Frame position where the drop occurred (if any).
    pub drop_frame: Option<i64>,
}

// ── Widget ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
enum DragMode {
    Move { start_frame_start: i64 },
    TrimIn {
        start_frame_start: i64,
        source_offset_start: usize,
        frame_count_start: usize,
    },
    TrimOut {
        frame_count_start: usize,
    },
}

struct DragState {
    clip_idx: usize,
    pointer_start_x: f32,
    mode: DragMode,
}

pub struct TimelinePanel {
    pub clips: Vec<TimelineClip>,
    pub playhead: i64,
    /// Pixels per frame.
    pub zoom: f32,
    scroll_offset: f32,
    drag: Option<DragState>,
    next_id: u64,
}

impl Default for TimelinePanel {
    fn default() -> Self {
        Self::new()
    }
}

impl TimelinePanel {
    pub fn new() -> Self {
        Self {
            clips: vec![],
            playhead: 0,
            zoom: 4.0,
            scroll_offset: 0.0,
            drag: None,
            next_id: 0,
        }
    }

    pub fn next_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Return the index of the currently selected clip, if any.
    pub fn selected_idx(&self) -> Option<usize> {
        self.clips.iter().position(|c| c.selected)
    }

    /// Total timeline duration in frames.
    pub fn total_frame_count(&self) -> usize {
        self.clips
            .iter()
            .map(|c| c.end_frame())
            .max()
            .unwrap_or(0) as usize
    }

    /// Return clips sorted by start_frame.
    pub fn sorted_clips(&self) -> Vec<&TimelineClip> {
        let mut sorted: Vec<_> = self.clips.iter().collect();
        sorted.sort_by_key(|c| c.start_frame);
        sorted
    }

    /// Find which clip (if any) contains the playhead and the local frame inside it.
    pub fn clip_at_playhead(&self) -> Option<(usize, usize)> {
        let ph = self.playhead;
        for (i, clip) in self.clips.iter().enumerate() {
            if let Some(local) = clip.local_frame_at(ph) {
                return Some((i, local));
            }
        }
        None
    }

    /// Draw the timeline and handle interaction.
    pub fn show(&mut self, ui: &mut egui::Ui) -> TimelineResponse {
        const RULER_H: f32 = 20.0;
        const TRACK_H: f32 = 64.0;
        const TOTAL_H: f32 = RULER_H + TRACK_H;
        const HANDLE_W: f32 = 8.0;

        let available_w = ui.available_width();
        let (rect, timeline_response) =
            ui.allocate_exact_size(Vec2::new(available_w, TOTAL_H), Sense::hover());

        let painter = ui.painter_at(rect);

        // ── Backgrounds ──────────────────────────────────────────────────────
        painter.rect_filled(rect, 0.0, Color32::from_gray(28));
        let ruler_rect =
            Rect::from_min_size(rect.min, Vec2::new(rect.width(), RULER_H));
        painter.rect_filled(ruler_rect, 0.0, Color32::from_gray(45));
        let track_rect =
            Rect::from_min_max(Pos2::new(rect.left(), rect.top() + RULER_H), rect.max);
        painter.rect_filled(track_rect, 0.0, Color32::from_gray(33));

        // ── Ruler ────────────────────────────────────────────────────────────
        let step = self.nice_step();
        let first = (self.scroll_offset / self.zoom) as i64 / step * step;
        let last = ((self.scroll_offset + rect.width()) / self.zoom) as i64 + step;
        let mut f = first;
        while f <= last {
            let x = self.frame_to_x(rect.left(), f);
            painter.line_segment(
                [Pos2::new(x, ruler_rect.bottom() - 6.0), Pos2::new(x, ruler_rect.bottom())],
                Stroke::new(1.0, Color32::from_gray(120)),
            );
            if self.zoom * step as f32 >= 30.0 {
                painter.text(
                    Pos2::new(x + 2.0, ruler_rect.top() + 4.0),
                    egui::Align2::LEFT_TOP,
                    f.to_string(),
                    FontId::monospace(9.0),
                    Color32::from_gray(170),
                );
            }
            f += step;
        }

        // ── Input ────────────────────────────────────────────────────────────
        let (pointer_pos, pressed, released, down, _p_delta, scroll, ctrl) =
            ui.input(|i| {
                (
                    i.pointer.interact_pos(),
                    i.pointer.button_pressed(egui::PointerButton::Primary),
                    i.pointer.button_released(egui::PointerButton::Primary),
                    i.pointer.button_down(egui::PointerButton::Primary),
                    i.pointer.delta(),
                    i.smooth_scroll_delta,
                    i.modifiers.ctrl,
                )
            });

        // Scroll / zoom when pointer is inside the timeline
        if pointer_pos.map_or(false, |p| rect.contains(p)) {
            if ctrl {
                self.zoom = (self.zoom + scroll.y * 0.15).clamp(0.5, 80.0);
            } else {
                self.scroll_offset =
                    (self.scroll_offset - scroll.x - scroll.y).max(0.0);
            }
        }

        // ── Drag-and-drop drop detection ─────────────────────────────────────
        let dropped_pool_idx = timeline_response
            .dnd_release_payload::<usize>()
            .map(|arc| *arc);
        let drop_frame = if let (Some(_idx), Some(pos)) =
            (dropped_pool_idx, pointer_pos)
        {
            if rect.contains(pos) {
                Some(self.x_to_frame(rect.left(), pos.x).max(0))
            } else {
                None
            }
        } else {
            None
        };

        // ── Hit-test clips in track area; playhead click in ruler ────────────
        let mut clicked_clip: Option<usize> = None;
        let mut clicked_background = false;
        let mut hovered_edge = false;

        if let Some(pos) = pointer_pos {
            if pressed {
                if ruler_rect.contains(pos) {
                    self.playhead = self.x_to_frame(rect.left(), pos.x).max(0);
                } else if track_rect.contains(pos) {
                    for (i, clip) in self.clips.iter().enumerate() {
                        let cl = self.frame_to_x(rect.left(), clip.start_frame);
                        let cr = self.frame_to_x(rect.left(), clip.end_frame());
                        if pos.x >= cl && pos.x <= cr {
                            clicked_clip = Some(i);
                            let edge_w = HANDLE_W.min((cr - cl) / 3.0);
                            let mode = if pos.x < cl + edge_w {
                                DragMode::TrimIn {
                                    start_frame_start: clip.start_frame,
                                    source_offset_start: clip.source_offset,
                                    frame_count_start: clip.frame_count,
                                }
                            } else if pos.x > cr - edge_w {
                                DragMode::TrimOut {
                                    frame_count_start: clip.frame_count,
                                }
                            } else {
                                DragMode::Move {
                                    start_frame_start: clip.start_frame,
                                }
                            };
                            self.drag = Some(DragState {
                                clip_idx: i,
                                pointer_start_x: pos.x,
                                mode,
                            });
                            break;
                        }
                    }
                    if clicked_clip.is_none() {
                        clicked_background = true;
                        self.playhead = self.x_to_frame(rect.left(), pos.x).max(0);
                    }
                }
            } else {
                // Hover cursor feedback
                if track_rect.contains(pos) {
                    for clip in &self.clips {
                        let cl = self.frame_to_x(rect.left(), clip.start_frame);
                        let cr = self.frame_to_x(rect.left(), clip.end_frame());
                        if pos.x >= cl && pos.x <= cr {
                            let edge_w = HANDLE_W.min((cr - cl) / 3.0);
                            if pos.x < cl + edge_w || pos.x > cr - edge_w {
                                hovered_edge = true;
                            }
                            break;
                        }
                    }
                }
            }
        }

        if hovered_edge {
            ui.output_mut(|o| o.cursor_icon = CursorIcon::ResizeHorizontal);
        }

        // ── Drag / trim / move logic ─────────────────────────────────────────
        if down {
            if let Some(ref ds) = self.drag {
                if let Some(pos) = pointer_pos {
                    let dx = pos.x - ds.pointer_start_x;
                    let delta_frames = (dx / self.zoom).round() as i64;
                    let idx = ds.clip_idx;
                    match &ds.mode {
                        DragMode::Move { start_frame_start } => {
                            let mut new_start = *start_frame_start + delta_frames;
                            new_start = new_start.max(0);
                            let fc = self.clips[idx].frame_count;
                            new_start = self.snap_start_frame(new_start, fc, Some(idx));
                            self.clips[idx].start_frame = new_start;
                        }
                        DragMode::TrimIn {
                            start_frame_start,
                            source_offset_start,
                            frame_count_start,
                        } => {
                            let total = self.clips[idx].source_frame_count as i64;
                            let new_source_offset =
                                (*source_offset_start as i64 + delta_frames).clamp(0, total - 1) as usize;
                            let new_start_frame =
                                (*start_frame_start + delta_frames).max(0);
                            let new_frame_count = (*frame_count_start as i64 - delta_frames)
                                .clamp(1, total - new_source_offset as i64) as usize;
                            let clip = &mut self.clips[idx];
                            clip.source_offset = new_source_offset;
                            clip.start_frame = new_start_frame;
                            clip.frame_count = new_frame_count;
                        }
                        DragMode::TrimOut { frame_count_start } => {
                            let total = self.clips[idx].source_frame_count as i64;
                            let source_offset = self.clips[idx].source_offset as i64;
                            let new_frame_count = (*frame_count_start as i64 + delta_frames)
                                .clamp(1, total - source_offset) as usize;
                            self.clips[idx].frame_count = new_frame_count;
                        }
                    }
                }
            }
        } else if released {
            if self.drag.is_some() {
                self.clips.sort_by_key(|c| c.start_frame);
            }
            self.drag = None;
        }

        // Update selection
        if let Some(i) = clicked_clip {
            for (j, c) in self.clips.iter_mut().enumerate() {
                c.selected = j == i;
            }
        } else if clicked_background {
            for c in self.clips.iter_mut() {
                c.selected = false;
            }
        }

        // ── Draw clips ───────────────────────────────────────────────────────
        for clip in &self.clips {
            let cl = self.frame_to_x(rect.left(), clip.start_frame);
            let cr = self.frame_to_x(rect.left(), clip.end_frame());

            if cr < rect.left() || cl > rect.right() {
                continue;
            }

            let clip_rect = Rect::from_min_max(
                Pos2::new(cl.max(rect.left()), track_rect.top() + 2.0),
                Pos2::new(cr.min(rect.right()), track_rect.bottom() - 2.0),
            );

            let base_color = if clip.selected {
                clip.color
            } else {
                clip.color.gamma_multiply(0.8)
            };
            painter.rect_filled(clip_rect, 4.0, base_color);

            // Trim indicator (darker stripe on left if head is trimmed)
            if clip.source_offset > 0 && clip_rect.width() > 2.0 {
                let hatch_w = 6.0f32.min(clip_rect.width());
                let hatch_rect =
                    Rect::from_min_size(clip_rect.min, Vec2::new(hatch_w, clip_rect.height()));
                painter.rect_filled(
                    hatch_rect,
                    4.0,
                    Color32::from_rgba_premultiplied(0, 0, 0, 120),
                );
            }

            // Leading-keyframe drop indicator (cross-hatch the first few pixels)
            if clip.drop_leading_keyframe && clip_rect.width() > 2.0 {
                let hatch_w = 6.0f32.min(clip_rect.width());
                let hatch_rect =
                    Rect::from_min_size(clip_rect.min, Vec2::new(hatch_w, clip_rect.height()));
                painter.rect_filled(
                    hatch_rect,
                    4.0,
                    Color32::from_rgba_premultiplied(60, 20, 20, 160),
                );
            }

            // Keyframe marker (only if first visible frame is the source keyframe)
            if clip.source_offset == 0 {
                let x = clip_rect.left();
                if x >= clip_rect.left() && x <= clip_rect.right() {
                    painter.line_segment(
                        [
                            Pos2::new(x, clip_rect.top()),
                            Pos2::new(x, clip_rect.bottom()),
                        ],
                        Stroke::new(1.5, Color32::from_rgb(240, 70, 70)),
                    );
                }
            }

            // Resize handles
            let handle_h = 12.0f32.min(clip_rect.height() - 4.0);
            let handle_y = clip_rect.top() + (clip_rect.height() - handle_h) / 2.0;
            let handle_w = 4.0f32.min(clip_rect.width() / 2.0);
            if handle_w > 1.0 {
                // left handle
                painter.rect_filled(
                    Rect::from_min_size(
                        Pos2::new(clip_rect.left() + 2.0, handle_y),
                        Vec2::new(handle_w, handle_h),
                    ),
                    2.0,
                    Color32::from_rgba_premultiplied(255, 255, 255, 180),
                );
                // right handle
                painter.rect_filled(
                    Rect::from_min_size(
                        Pos2::new(clip_rect.right() - 2.0 - handle_w, handle_y),
                        Vec2::new(handle_w, handle_h),
                    ),
                    2.0,
                    Color32::from_rgba_premultiplied(255, 255, 255, 180),
                );
            }

            // Border
            let bw = if clip.selected { 2.0 } else { 1.0 };
            let bc = if clip.selected {
                Color32::WHITE
            } else {
                Color32::from_gray(140)
            };
            painter.rect_stroke(clip_rect, 4.0, Stroke::new(bw, bc));

            // Label
            if clip_rect.width() > 10.0 {
                painter.text(
                    Pos2::new(
                        clip_rect.left().max(rect.left()) + 4.0,
                        clip_rect.top() + 6.0,
                    ),
                    egui::Align2::LEFT_TOP,
                    &clip.name,
                    FontId::proportional(11.0),
                    Color32::WHITE,
                );
            }
        }

        // ── Playhead ─────────────────────────────────────────────────────────
        let ph_x = self.frame_to_x(rect.left(), self.playhead);
        if ph_x >= rect.left() && ph_x <= rect.right() {
            painter.line_segment(
                [Pos2::new(ph_x, rect.top()), Pos2::new(ph_x, rect.bottom())],
                Stroke::new(2.0, Color32::from_rgb(255, 210, 0)),
            );
            // Triangle handle at top
            painter.add(egui::Shape::convex_polygon(
                vec![
                    Pos2::new(ph_x, ruler_rect.top() + 2.0),
                    Pos2::new(ph_x - 5.0, ruler_rect.top() + 11.0),
                    Pos2::new(ph_x + 5.0, ruler_rect.top() + 11.0),
                ],
                Color32::from_rgb(255, 210, 0),
                Stroke::NONE,
            ));
        }

        TimelineResponse {
            playhead: self.playhead,
            selected_idx: self.clips.iter().position(|c| c.selected),
            dropped_pool_idx,
            drop_frame,
        }
    }

    // ── Helpers ──────────────────────────────────────────────────────────────

    fn frame_to_x(&self, left: f32, frame: i64) -> f32 {
        left + frame as f32 * self.zoom - self.scroll_offset
    }

    fn x_to_frame(&self, left: f32, x: f32) -> i64 {
        ((x - left + self.scroll_offset) / self.zoom) as i64
    }

    fn nice_step(&self) -> i64 {
        let target_px = 40.0;
        let raw = target_px / self.zoom;
        let candidates = [1, 2, 5, 10, 25, 50, 100, 250, 500, 1000];
        *candidates.iter().find(|&&s| s as f32 >= raw).unwrap_or(&1000)
    }

    /// Snap a candidate start frame to the nearest edge of another clip.
    pub fn snap_start_frame(&self, mut new_start: i64, frame_count: usize, exclude_idx: Option<usize>) -> i64 {
        let threshold = ((12.0 / self.zoom).round() as i64).max(1);
        let new_end = new_start + frame_count as i64;

        let mut best_snap: Option<(i64, i64)> = None; // (distance, snapped_start)

        for (i, other) in self.clips.iter().enumerate() {
            if exclude_idx == Some(i) {
                continue;
            }
            let edges = [other.start_frame, other.end_frame()];
            for &edge in &edges {
                // snap start to edge
                let d = (new_start - edge).abs();
                if d <= threshold {
                    if best_snap.map_or(true, |(bd, _)| d < bd) {
                        best_snap = Some((d, edge));
                    }
                }
                // snap end to edge
                let d = (new_end - edge).abs();
                if d <= threshold {
                    let snapped_start = edge - frame_count as i64;
                    if snapped_start >= 0 {
                        if best_snap.map_or(true, |(bd, _)| d < bd) {
                            best_snap = Some((d, snapped_start));
                        }
                    }
                }
            }
        }

        if let Some((_, snapped)) = best_snap {
            new_start = snapped;
        }
        new_start
    }
}
