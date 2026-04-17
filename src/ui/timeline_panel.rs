use eframe::egui::{self, Color32, FontId, Pos2, Rect, Sense, Stroke, Vec2};

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
    pub frame_count: usize,
    /// Timeline start position in *frame* units.
    pub start_frame: i64,
    pub color: Color32,
    pub selected: bool,
    /// When true, the first keyframe of this clip is dropped on playback,
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
        Some((timeline_frame - self.start_frame) as usize)
    }
}

// ── Response ─────────────────────────────────────────────────────────────────

pub struct TimelineResponse {
    pub playhead: i64,
    pub selected_idx: Option<usize>,
}

// ── Widget ───────────────────────────────────────────────────────────────────

struct DragState {
    clip_idx: usize,
    drag_start_frame: i64,
    pointer_start_x: f32,
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

        let available_w = ui.available_width();
        let (rect, _) = ui.allocate_exact_size(Vec2::new(available_w, TOTAL_H), Sense::hover());

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

        // Hit-test clips in track area; playhead click in ruler
        let mut clicked_clip: Option<usize> = None;
        let mut clicked_background = false;

        if let Some(pos) = pointer_pos {
            if pressed {
                if ruler_rect.contains(pos) {
                    // Click in ruler → move playhead
                    self.playhead = self.x_to_frame(rect.left(), pos.x).max(0);
                } else if track_rect.contains(pos) {
                    for (i, clip) in self.clips.iter().enumerate() {
                        let cl = self.frame_to_x(rect.left(), clip.start_frame);
                        let cr = self.frame_to_x(rect.left(), clip.end_frame());
                        if pos.x >= cl && pos.x <= cr {
                            clicked_clip = Some(i);
                            self.drag = Some(DragState {
                                clip_idx: i,
                                drag_start_frame: clip.start_frame,
                                pointer_start_x: pos.x,
                            });
                            break;
                        }
                    }
                    if clicked_clip.is_none() {
                        clicked_background = true;
                        self.playhead =
                            self.x_to_frame(rect.left(), pos.x).max(0);
                    }
                }
            }
        }

        // Drag clip
        if down {
            if let Some(ref ds) = self.drag {
                if let Some(pos) = pointer_pos {
                    let dx = pos.x - ds.pointer_start_x;
                    let new_start =
                        ds.drag_start_frame + (dx / self.zoom) as i64;
                    self.clips[ds.clip_idx].start_frame = new_start.max(0);
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

            // Leading-keyframe drop indicator (cross-hatch the first few pixels)
            if clip.drop_leading_keyframe && clip_rect.width() > 2.0 {
                let hatch_w = 6.0f32.min(clip_rect.width());
                let hatch_rect = Rect::from_min_size(clip_rect.min, Vec2::new(hatch_w, clip_rect.height()));
                painter.rect_filled(hatch_rect, 4.0, Color32::from_rgba_premultiplied(0, 0, 0, 120));
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
}
