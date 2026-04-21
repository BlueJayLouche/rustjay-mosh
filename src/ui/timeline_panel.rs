use eframe::egui::{self, Color32, FontId, Pos2, Rect, Sense, Stroke, Vec2};
use eframe::egui::CursorIcon;

use crate::audio::{AudioClip, AudioTimelineClip};

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

// ── Timecode helper ──────────────────────────────────────────────────────────

fn format_timecode(frame: i64, fps: u32) -> String {
    let fps = fps as i64;
    let ff = frame.rem_euclid(fps);
    let ss = (frame / fps).rem_euclid(60);
    let mm = (frame / (fps * 60)).rem_euclid(60);
    let hh = frame / (fps * 3600);
    format!("{:02}:{:02}:{:02}:{:02}", hh, mm, ss, ff)
}

// ── Data ─────────────────────────────────────────────────────────────────────

/// A video clip placed on the timeline track.
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
    /// Video track index. 0 = top (render priority), 1 = bottom.
    pub track: u8,
}

impl TimelineClip {
    pub fn end_frame(&self) -> i64 {
        self.start_frame + self.frame_count as i64
    }

    pub fn local_frame_at(&self, timeline_frame: i64) -> Option<usize> {
        if timeline_frame < self.start_frame || timeline_frame >= self.end_frame() {
            return None;
        }
        Some((timeline_frame - self.start_frame + self.source_offset as i64) as usize)
    }
}

// ── Response ─────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug)]
pub enum PoolDragPayload {
    Video(usize),
    Audio(usize),
}

pub struct TimelineResponse {
    pub playhead: i64,
    pub selected_video_idx: Option<usize>,
    pub selected_audio_idx: Option<usize>,
    /// If a pool item was dropped onto the timeline, this is its payload.
    pub dropped_payload: Option<PoolDragPayload>,
    /// Frame position where the drop occurred (if any).
    pub drop_frame: Option<i64>,
    /// True if the drop landed in the audio lane.
    pub drop_is_audio: bool,
    /// Video track the drop landed on (0 = top, 1 = bottom). Only meaningful
    /// when `drop_is_audio` is false.
    pub drop_track: u8,
}

// ── Widget ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum DragTarget {
    Video,
    Audio,
}

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
    FadeIn {
        fade_in_start: usize,
    },
    FadeOut {
        fade_out_start: usize,
    },
}

struct DragState {
    target: DragTarget,
    clip_idx: usize,
    pointer_start_x: f32,
    mode: DragMode,
}

pub struct TimelinePanel {
    pub clips: Vec<TimelineClip>,
    pub audio_clips: Vec<AudioTimelineClip>,
    pub playhead: i64,
    /// Pixels per frame.
    pub zoom: f32,
    scroll_offset: f32,
    drag: Option<DragState>,
    next_id: u64,
    /// Inclusive-exclusive selection range in timeline frames. Used as the
    /// effect-application range when present.
    pub selection: Option<(i64, i64)>,
    /// Anchor frame while the user is dragging a new selection on the ruler.
    ruler_drag_anchor: Option<i64>,
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
            audio_clips: vec![],
            playhead: 0,
            zoom: 4.0,
            scroll_offset: 0.0,
            drag: None,
            next_id: 0,
            selection: None,
            ruler_drag_anchor: None,
        }
    }

    pub fn clear_selection(&mut self) {
        self.selection = None;
    }

    pub fn next_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    pub fn selected_video_idx(&self) -> Option<usize> {
        self.clips.iter().position(|c| c.selected)
    }

    pub fn selected_audio_idx(&self) -> Option<usize> {
        self.audio_clips.iter().position(|c| c.selected)
    }

    pub fn total_frame_count(&self) -> usize {
        let v = self.clips.iter().map(|c| c.end_frame()).max().unwrap_or(0);
        let a: i64 = self.audio_clips.iter().map(|c: &AudioTimelineClip| c.end_frame()).max().unwrap_or(0);
        v.max(a) as usize
    }

    pub fn sorted_clips(&self) -> Vec<&TimelineClip> {
        let mut sorted: Vec<_> = self.clips.iter().collect();
        sorted.sort_by_key(|c| c.start_frame);
        sorted
    }

    pub fn sorted_audio_clips(&self) -> Vec<&AudioTimelineClip> {
        let mut sorted: Vec<_> = self.audio_clips.iter().collect();
        sorted.sort_by_key(|c| c.start_frame);
        sorted
    }

    pub fn clip_at_playhead(&self) -> Option<(usize, usize)> {
        let ph = self.playhead;
        // Top track has render priority, then fall through to bottom.
        for track in 0..=1u8 {
            for (i, clip) in self.clips.iter().enumerate() {
                if clip.track != track {
                    continue;
                }
                if let Some(local) = clip.local_frame_at(ph) {
                    return Some((i, local));
                }
            }
        }
        None
    }

    pub fn show(&mut self, ui: &mut egui::Ui, fps: u32, audio_sources: &[AudioClip]) -> TimelineResponse {
        const RULER_H: f32 = 22.0;
        const TRACK_H: f32 = 34.0;
        const VIDEO_H: f32 = TRACK_H * 2.0;
        const AUDIO_H: f32 = 48.0;
        const TOTAL_H: f32 = RULER_H + VIDEO_H + AUDIO_H;
        const HANDLE_W: f32 = 8.0;

        let available_w = ui.available_width();
        let (rect, timeline_response) =
            ui.allocate_exact_size(Vec2::new(available_w, TOTAL_H), Sense::hover());

        let painter = ui.painter_at(rect);

        // Layout rects
        let ruler_rect = Rect::from_min_size(rect.min, Vec2::new(rect.width(), RULER_H));
        let video_rect = Rect::from_min_max(
            Pos2::new(rect.left(), rect.top() + RULER_H),
            Pos2::new(rect.right(), rect.top() + RULER_H + VIDEO_H),
        );
        let track_rects = [
            Rect::from_min_max(
                Pos2::new(video_rect.left(), video_rect.top()),
                Pos2::new(video_rect.right(), video_rect.top() + TRACK_H),
            ),
            Rect::from_min_max(
                Pos2::new(video_rect.left(), video_rect.top() + TRACK_H),
                Pos2::new(video_rect.right(), video_rect.bottom()),
            ),
        ];
        let audio_rect = Rect::from_min_max(
            Pos2::new(rect.left(), rect.top() + RULER_H + VIDEO_H),
            rect.max,
        );

        // Backgrounds
        painter.rect_filled(rect, 0.0, Color32::from_gray(28));
        painter.rect_filled(ruler_rect, 0.0, Color32::from_gray(45));
        painter.rect_filled(track_rects[0], 0.0, Color32::from_gray(36));
        painter.rect_filled(track_rects[1], 0.0, Color32::from_gray(30));
        // 1px divider between the two video tracks
        painter.line_segment(
            [
                Pos2::new(video_rect.left(), video_rect.top() + TRACK_H),
                Pos2::new(video_rect.right(), video_rect.top() + TRACK_H),
            ],
            Stroke::new(1.0, Color32::from_gray(20)),
        );
        painter.rect_filled(audio_rect, 0.0, Color32::from_gray(38));

        // Track labels
        painter.text(
            Pos2::new(video_rect.left() + 4.0, track_rects[0].top() + 2.0),
            egui::Align2::LEFT_TOP,
            "V1",
            FontId::monospace(9.0),
            Color32::from_gray(180),
        );
        painter.text(
            Pos2::new(video_rect.left() + 4.0, track_rects[1].top() + 2.0),
            egui::Align2::LEFT_TOP,
            "V2",
            FontId::monospace(9.0),
            Color32::from_gray(180),
        );

        // Selection band — painted under clips so clip bodies stay readable.
        if let Some((sel_a, sel_b)) = self.selection {
            let x1 = self.frame_to_x(rect.left(), sel_a);
            let x2 = self.frame_to_x(rect.left(), sel_b);
            let lx = x1.min(x2).max(rect.left());
            let rx = x1.max(x2).min(rect.right());
            if rx > lx {
                let band = Rect::from_min_max(
                    Pos2::new(lx, video_rect.top()),
                    Pos2::new(rx, audio_rect.bottom()),
                );
                painter.rect_filled(
                    band,
                    0.0,
                    Color32::from_rgba_premultiplied(255, 210, 0, 36),
                );
                // Top ribbon inside the ruler
                let ribbon = Rect::from_min_max(
                    Pos2::new(lx, ruler_rect.top() + 2.0),
                    Pos2::new(rx, ruler_rect.top() + 6.0),
                );
                painter.rect_filled(ribbon, 0.0, Color32::from_rgb(255, 210, 0));
                // Vertical edges
                painter.line_segment(
                    [Pos2::new(lx, ruler_rect.top()), Pos2::new(lx, audio_rect.bottom())],
                    Stroke::new(1.0, Color32::from_rgba_premultiplied(255, 210, 0, 180)),
                );
                painter.line_segment(
                    [Pos2::new(rx, ruler_rect.top()), Pos2::new(rx, audio_rect.bottom())],
                    Stroke::new(1.0, Color32::from_rgba_premultiplied(255, 210, 0, 180)),
                );
            }
        }

        // ── Ruler ────────────────────────────────────────────────────────────
        let step = self.nice_step(fps);
        let first = (self.scroll_offset / self.zoom) as i64 / step * step;
        let last = ((self.scroll_offset + rect.width()) / self.zoom) as i64 + step;
        let mut f = first;
        while f <= last {
            let x = self.frame_to_x(rect.left(), f);
            let is_second = (f % fps as i64) == 0;
            let tick_h = if is_second { 10.0 } else { 6.0 };
            painter.line_segment(
                [Pos2::new(x, ruler_rect.bottom() - tick_h), Pos2::new(x, ruler_rect.bottom())],
                Stroke::new(1.0, Color32::from_gray(120)),
            );
            if self.zoom * step as f32 >= 30.0 {
                let tc = if is_second {
                    format_timecode(f, fps)
                } else {
                    format!("{:02}", f.rem_euclid(fps as i64))
                };
                painter.text(
                    Pos2::new(x + 2.0, ruler_rect.top() + 2.0),
                    egui::Align2::LEFT_TOP,
                    tc,
                    FontId::monospace(if is_second { 10.0 } else { 8.0 }),
                    if is_second { Color32::from_gray(220) } else { Color32::from_gray(160) },
                );
            }
            f += step;
        }

        // ── Input ────────────────────────────────────────────────────────────
        let (pointer_pos, pressed, released, down, _p_delta, scroll, ctrl, shift) = ui.input(|i| {
            (
                i.pointer.interact_pos(),
                i.pointer.button_pressed(egui::PointerButton::Primary),
                i.pointer.button_released(egui::PointerButton::Primary),
                i.pointer.button_down(egui::PointerButton::Primary),
                i.pointer.delta(),
                i.smooth_scroll_delta,
                i.modifiers.ctrl,
                i.modifiers.shift,
            )
        });

        if pointer_pos.map_or(false, |p| rect.contains(p)) {
            if ctrl {
                // Multiplicative step so ctrl-scroll feels smooth across the
                // full log range (0.02 ≈ 10 min in a 600px view, up to 500).
                let factor = (1.0 + scroll.y * 0.01).clamp(0.5, 2.0);
                self.zoom = (self.zoom * factor).clamp(0.02, 500.0);
            } else {
                self.scroll_offset = (self.scroll_offset - scroll.x - scroll.y).max(0.0);
            }
        }

        // ── Drag-and-drop drop detection ─────────────────────────────────────
        let dropped_payload = timeline_response
            .dnd_release_payload::<PoolDragPayload>()
            .map(|arc| *arc);
        let (drop_frame, drop_is_audio, drop_track) =
            if let (Some(_), Some(pos)) = (dropped_payload, pointer_pos) {
                if rect.contains(pos) {
                    let frame = self.x_to_frame(rect.left(), pos.x).max(0);
                    let is_audio = audio_rect.contains(pos);
                    let track: u8 = if track_rects[1].contains(pos) { 1 } else { 0 };
                    (Some(frame), is_audio, track)
                } else {
                    (None, false, 0)
                }
            } else {
                (None, false, 0)
            };

        // ── Hit-test ─────────────────────────────────────────────────────────
        let mut clicked_video: Option<usize> = None;
        let mut clicked_audio: Option<usize> = None;
        let mut clicked_background = false;
        let mut hovered_edge = false;

        // Ruler drag selection — update before the clip hit-test block so a
        // drag that started on the ruler keeps going even if the pointer
        // leaves it.
        if let Some(pos) = pointer_pos {
            if pressed && ruler_rect.contains(pos) {
                let f = self.x_to_frame(rect.left(), pos.x).max(0);
                self.playhead = f;
                self.selection = None;
                self.ruler_drag_anchor = Some(f);
            } else if down && self.ruler_drag_anchor.is_some() {
                let anchor = self.ruler_drag_anchor.unwrap();
                let cur = self.x_to_frame(rect.left(), pos.x).max(0);
                if cur != anchor {
                    let (lo, hi) = if cur >= anchor { (anchor, cur) } else { (cur, anchor) };
                    self.selection = Some((lo, hi));
                    self.playhead = cur;
                }
            }
        }
        if released && self.ruler_drag_anchor.take().is_some() {
            // A plain click with no drag leaves `selection` as None, which is
            // the right state. Nothing else to do.
        }

        if let Some(pos) = pointer_pos {
            if pressed {
                if ruler_rect.contains(pos) {
                    // Already handled above — playhead + selection set.
                } else if video_rect.contains(pos) {
                    let hit_track: u8 = if track_rects[1].contains(pos) { 1 } else { 0 };
                    for (i, clip) in self.clips.iter().enumerate() {
                        if clip.track != hit_track {
                            continue;
                        }
                        let cl = self.frame_to_x(rect.left(), clip.start_frame);
                        let cr = self.frame_to_x(rect.left(), clip.end_frame());
                        if pos.x >= cl && pos.x <= cr {
                            clicked_video = Some(i);
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
                                target: DragTarget::Video,
                                clip_idx: i,
                                pointer_start_x: pos.x,
                                mode,
                            });
                            break;
                        }
                    }
                    if clicked_video.is_none() {
                        clicked_background = true;
                        self.playhead = self.x_to_frame(rect.left(), pos.x).max(0);
                    }
                } else if audio_rect.contains(pos) {
                    for (i, clip) in self.audio_clips.iter().enumerate() {
                        let cl = self.frame_to_x(rect.left(), clip.start_frame);
                        let cr = self.frame_to_x(rect.left(), clip.end_frame());
                        if pos.x >= cl && pos.x <= cr {
                            clicked_audio = Some(i);
                            let edge_w = HANDLE_W.min((cr - cl) / 3.0);
                            let mode = if shift {
                                // Shift+drag on left/right half controls fades
                                if pos.x < (cl + cr) / 2.0 {
                                    DragMode::FadeIn {
                                        fade_in_start: clip.fade_in_frames,
                                    }
                                } else {
                                    DragMode::FadeOut {
                                        fade_out_start: clip.fade_out_frames,
                                    }
                                }
                            } else if pos.x < cl + edge_w {
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
                                target: DragTarget::Audio,
                                clip_idx: i,
                                pointer_start_x: pos.x,
                                mode,
                            });
                            break;
                        }
                    }
                    if clicked_audio.is_none() {
                        clicked_background = true;
                        self.playhead = self.x_to_frame(rect.left(), pos.x).max(0);
                    }
                }
            } else {
                // hover cursor feedback
                if video_rect.contains(pos) {
                    let hit_track: u8 = if track_rects[1].contains(pos) { 1 } else { 0 };
                    for clip in &self.clips {
                        if clip.track != hit_track {
                            continue;
                        }
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
                } else if audio_rect.contains(pos) {
                    for clip in &self.audio_clips {
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

        // ── Drag logic ───────────────────────────────────────────────────────
        if down {
            if let Some(ref ds) = self.drag {
                if let Some(pos) = pointer_pos {
                    let dx = pos.x - ds.pointer_start_x;
                    let delta_frames = (dx / self.zoom).round() as i64;
                    let idx = ds.clip_idx;
                    match ds.target {
                        DragTarget::Video => {
                            match &ds.mode {
                                DragMode::Move { start_frame_start } => {
                                    let mut new_start = *start_frame_start + delta_frames;
                                    new_start = new_start.max(0);
                                    let fc = self.clips[idx].frame_count;
                                    new_start = self.snap_start_frame(new_start, fc, Some((DragTarget::Video, idx)));
                                    self.clips[idx].start_frame = new_start;
                                }
                                DragMode::TrimIn { start_frame_start, source_offset_start, frame_count_start } => {
                                    let total = self.clips[idx].source_frame_count as i64;
                                    let new_source_offset = (*source_offset_start as i64 + delta_frames).clamp(0, total - 1) as usize;
                                    let new_start_frame = (*start_frame_start + delta_frames).max(0);
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
                                DragMode::FadeIn { .. } | DragMode::FadeOut { .. } => {}
                            }
                        }
                        DragTarget::Audio => {
                            match &ds.mode {
                                DragMode::Move { start_frame_start } => {
                                    let mut new_start = *start_frame_start + delta_frames;
                                    new_start = new_start.max(0);
                                    let fc = self.audio_clips[idx].frame_count;
                                    new_start = self.snap_start_frame(new_start, fc, Some((DragTarget::Audio, idx)));
                                    self.audio_clips[idx].start_frame = new_start;
                                }
                                DragMode::TrimIn { start_frame_start, source_offset_start, frame_count_start } => {
                                    let total = audio_sources[self.audio_clips[idx].audio_clip_idx].peaks.len() as i64;
                                    let new_source_offset = (*source_offset_start as i64 + delta_frames).clamp(0, total - 1) as usize;
                                    let new_start_frame = (*start_frame_start + delta_frames).max(0);
                                    let new_frame_count = (*frame_count_start as i64 - delta_frames)
                                        .clamp(1, total - new_source_offset as i64) as usize;
                                    let clip = &mut self.audio_clips[idx];
                                    clip.source_offset = new_source_offset;
                                    clip.start_frame = new_start_frame;
                                    clip.frame_count = new_frame_count;
                                }
                                DragMode::TrimOut { frame_count_start } => {
                                    let total = audio_sources[self.audio_clips[idx].audio_clip_idx].peaks.len() as i64;
                                    let source_offset = self.audio_clips[idx].source_offset as i64;
                                    let new_frame_count = (*frame_count_start as i64 + delta_frames)
                                        .clamp(1, total - source_offset) as usize;
                                    self.audio_clips[idx].frame_count = new_frame_count;
                                }
                                DragMode::FadeIn { fade_in_start } => {
                                    let delta = delta_frames.max(-(*fade_in_start as i64)) as usize;
                                    let max_fade = self.audio_clips[idx].frame_count / 2;
                                    self.audio_clips[idx].fade_in_frames = (*fade_in_start + delta).min(max_fade);
                                }
                                DragMode::FadeOut { fade_out_start } => {
                                    let delta = delta_frames.max(-(*fade_out_start as i64)) as usize;
                                    let max_fade = self.audio_clips[idx].frame_count / 2;
                                    self.audio_clips[idx].fade_out_frames = (*fade_out_start + delta).min(max_fade);
                                }
                            }
                        }
                    }
                }
            }
        } else if released {
            if self.drag.is_some() {
                self.clips.sort_by_key(|c| c.start_frame);
                self.audio_clips.sort_by_key(|c| c.start_frame);
                self.validate_mosh_state();
            }
            self.drag = None;
        }

        // Update selection
        if let Some(i) = clicked_video {
            for (j, c) in self.clips.iter_mut().enumerate() { c.selected = j == i; }
            for c in self.audio_clips.iter_mut() { c.selected = false; }
        } else if let Some(i) = clicked_audio {
            for c in self.clips.iter_mut() { c.selected = false; }
            for (j, c) in self.audio_clips.iter_mut().enumerate() { c.selected = j == i; }
        } else if clicked_background {
            for c in self.clips.iter_mut() { c.selected = false; }
            for c in self.audio_clips.iter_mut() { c.selected = false; }
        }

        // ── Draw video clips ─────────────────────────────────────────────────
        for clip in &self.clips {
            let cl = self.frame_to_x(rect.left(), clip.start_frame);
            let cr = self.frame_to_x(rect.left(), clip.end_frame());
            if cr < rect.left() || cl > rect.right() { continue; }

            let tr = track_rects[(clip.track as usize).min(1)];
            let clip_rect = Rect::from_min_max(
                Pos2::new(cl.max(rect.left()), tr.top() + 2.0),
                Pos2::new(cr.min(rect.right()), tr.bottom() - 2.0),
            );

            let base_color = if clip.selected { clip.color } else { clip.color.gamma_multiply(0.8) };
            painter.rect_filled(clip_rect, 4.0, base_color);

            if clip.source_offset > 0 && clip_rect.width() > 2.0 {
                let hatch = Rect::from_min_size(clip_rect.min, Vec2::new(6.0f32.min(clip_rect.width()), clip_rect.height()));
                painter.rect_filled(hatch, 4.0, Color32::from_rgba_premultiplied(0, 0, 0, 120));
            }
            if clip.drop_leading_keyframe && clip_rect.width() > 2.0 {
                let hatch = Rect::from_min_size(clip_rect.min, Vec2::new(6.0f32.min(clip_rect.width()), clip_rect.height()));
                painter.rect_filled(hatch, 4.0, Color32::from_rgba_premultiplied(60, 20, 20, 160));
            }
            if clip.source_offset == 0 {
                let x = clip_rect.left();
                painter.line_segment(
                    [Pos2::new(x, clip_rect.top()), Pos2::new(x, clip_rect.bottom())],
                    Stroke::new(1.5, Color32::from_rgb(240, 70, 70)),
                );
            }

            // handles
            let handle_h = 12.0f32.min(clip_rect.height() - 4.0);
            let handle_y = clip_rect.top() + (clip_rect.height() - handle_h) / 2.0;
            let handle_w = 4.0f32.min(clip_rect.width() / 2.0);
            if handle_w > 1.0 {
                painter.rect_filled(Rect::from_min_size(Pos2::new(clip_rect.left() + 2.0, handle_y), Vec2::new(handle_w, handle_h)), 2.0, Color32::from_rgba_premultiplied(255, 255, 255, 180));
                painter.rect_filled(Rect::from_min_size(Pos2::new(clip_rect.right() - 2.0 - handle_w, handle_y), Vec2::new(handle_w, handle_h)), 2.0, Color32::from_rgba_premultiplied(255, 255, 255, 180));
            }

            let bw = if clip.selected { 2.0 } else { 1.0 };
            let bc = if clip.selected { Color32::WHITE } else { Color32::from_gray(140) };
            painter.rect_stroke(clip_rect, 4.0, Stroke::new(bw, bc));

            if clip_rect.width() > 10.0 {
                painter.text(Pos2::new(clip_rect.left().max(rect.left()) + 4.0, clip_rect.top() + 6.0), egui::Align2::LEFT_TOP, &clip.name, FontId::proportional(11.0), Color32::WHITE);
            }
        }

        // ── Draw audio clips ─────────────────────────────────────────────────
        for (_i, clip) in self.audio_clips.iter().enumerate() {
            let cl = self.frame_to_x(rect.left(), clip.start_frame);
            let cr = self.frame_to_x(rect.left(), clip.end_frame());
            if cr < rect.left() || cl > rect.right() { continue; }

            let clip_rect = Rect::from_min_max(
                Pos2::new(cl.max(rect.left()), audio_rect.top() + 2.0),
                Pos2::new(cr.min(rect.right()), audio_rect.bottom() - 2.0),
            );

            let base_color = Color32::from_gray(if clip.selected { 70 } else { 55 });
            painter.rect_filled(clip_rect, 4.0, base_color);

            // Waveform bars
            if let Some(source) = audio_sources.get(clip.audio_clip_idx) {
                let peak_start = clip.source_offset;
                let peak_count = clip.frame_count.min(source.peaks.len().saturating_sub(peak_start));
                let mid_y = clip_rect.center().y;
                let max_bar_h = clip_rect.height() / 2.0 - 2.0;

                // Bucket peaks to pixels when zoom is low
                let px_per_bar = self.zoom.max(1.0);
                let bars = (clip_rect.width() / px_per_bar).ceil() as usize;
                for bar in 0..bars {
                    let bx = clip_rect.left() + bar as f32 * px_per_bar;
                    if bx > clip_rect.right() { break; }
                    let bwidth = px_per_bar.min(clip_rect.right() - bx);
                    let frame_start = (bar as f32 * px_per_bar / self.zoom) as usize;
                    let frame_end = (((bar + 1) as f32 * px_per_bar) / self.zoom) as usize;
                    let mut min = 0.0f32;
                    let mut max = 0.0f32;
                    for f_idx in frame_start..frame_end {
                        let p_idx = peak_start + f_idx;
                        if p_idx < source.peaks.len() && p_idx < peak_start + peak_count {
                            min = min.min(source.peaks[p_idx].min);
                            max = max.max(source.peaks[p_idx].max);
                        }
                    }
                    let h = (max - min).abs() * max_bar_h;
                    let h = h.clamp(1.0, max_bar_h * 2.0);
                    let bar_rect = Rect::from_min_size(
                        Pos2::new(bx, mid_y - h / 2.0),
                        Vec2::new(bwidth.max(1.0), h),
                    );
                    painter.rect_filled(bar_rect, 1.0, Color32::from_rgb(120, 200, 120));
                }
            }

            // Fade overlays
            let fade_in_w = (clip.fade_in_frames as f32 * self.zoom).min(clip_rect.width());
            let fade_out_w = (clip.fade_out_frames as f32 * self.zoom).min(clip_rect.width());
            if fade_in_w > 1.0 {
                painter.rect_filled(
                    Rect::from_min_size(clip_rect.min, Vec2::new(fade_in_w, clip_rect.height())),
                    4.0,
                    Color32::from_rgba_premultiplied(0, 0, 0, 80),
                );
            }
            if fade_out_w > 1.0 {
                painter.rect_filled(
                    Rect::from_min_max(Pos2::new(clip_rect.right() - fade_out_w, clip_rect.top()), clip_rect.max),
                    4.0,
                    Color32::from_rgba_premultiplied(0, 0, 0, 80),
                );
            }

            // handles
            let handle_h = 10.0f32.min(clip_rect.height() - 4.0);
            let handle_y = clip_rect.top() + (clip_rect.height() - handle_h) / 2.0;
            let handle_w = 4.0f32.min(clip_rect.width() / 2.0);
            if handle_w > 1.0 {
                painter.rect_filled(Rect::from_min_size(Pos2::new(clip_rect.left() + 2.0, handle_y), Vec2::new(handle_w, handle_h)), 2.0, Color32::from_rgba_premultiplied(255, 255, 255, 180));
                painter.rect_filled(Rect::from_min_size(Pos2::new(clip_rect.right() - 2.0 - handle_w, handle_y), Vec2::new(handle_w, handle_h)), 2.0, Color32::from_rgba_premultiplied(255, 255, 255, 180));
            }

            let bw = if clip.selected { 2.0 } else { 1.0 };
            let bc = if clip.selected { Color32::WHITE } else { Color32::from_gray(140) };
            painter.rect_stroke(clip_rect, 4.0, Stroke::new(bw, bc));

            if clip_rect.width() > 10.0 {
                if let Some(source) = audio_sources.get(clip.audio_clip_idx) {
                    painter.text(Pos2::new(clip_rect.left().max(rect.left()) + 4.0, clip_rect.top() + 4.0), egui::Align2::LEFT_TOP, &source.name, FontId::proportional(10.0), Color32::WHITE);
                }
            }
        }

        // ── Playhead ─────────────────────────────────────────────────────────
        let ph_x = self.frame_to_x(rect.left(), self.playhead);
        if ph_x >= rect.left() && ph_x <= rect.right() {
            painter.line_segment(
                [Pos2::new(ph_x, rect.top()), Pos2::new(ph_x, rect.bottom())],
                Stroke::new(2.0, Color32::from_rgb(255, 210, 0)),
            );
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
            selected_video_idx: self.clips.iter().position(|c| c.selected),
            selected_audio_idx: self.audio_clips.iter().position(|c| c.selected),
            dropped_payload,
            drop_frame,
            drop_is_audio,
            drop_track,
        }
    }

    fn frame_to_x(&self, left: f32, frame: i64) -> f32 {
        left + frame as f32 * self.zoom - self.scroll_offset
    }

    fn x_to_frame(&self, left: f32, x: f32) -> i64 {
        ((x - left + self.scroll_offset) / self.zoom) as i64
    }

    fn nice_step(&self, fps: u32) -> i64 {
        let target_px = 40.0;
        let raw = target_px / self.zoom;
        let fps = fps as i64;
        // Prefer steps that align with seconds when possible
        let candidates = if raw >= fps as f32 {
            vec![fps, fps * 2, fps * 5, fps * 10, fps * 30, fps * 60, fps * 120, fps * 300, fps * 600]
        } else {
            vec![1, 2, 5, 10, 25, fps, fps * 2, fps * 5, fps * 10, fps * 30, fps * 60]
        };
        *candidates.iter().find(|&&s| s as f32 >= raw).unwrap_or(&(fps * 600))
    }

    /// Snap a candidate start frame to the nearest edge of another clip (video or audio).
    pub fn snap_start_frame(&self, mut new_start: i64, frame_count: usize, exclude: Option<(DragTarget, usize)>) -> i64 {
        let threshold = ((12.0 / self.zoom).round() as i64).max(1);
        let new_end = new_start + frame_count as i64;
        let mut best_snap: Option<(i64, i64)> = None;

        for (i, other) in self.clips.iter().enumerate() {
            if exclude == Some((DragTarget::Video, i)) { continue; }
            for &edge in &[other.start_frame, other.end_frame()] {
                let d: i64 = (new_start - edge).abs();
                if d <= threshold && best_snap.map_or(true, |(bd, _)| d < bd) {
                    best_snap = Some((d, edge));
                }
                let d: i64 = (new_end - edge).abs();
                if d <= threshold {
                    let snapped = edge - frame_count as i64;
                    if snapped >= 0 && best_snap.map_or(true, |(bd, _)| d < bd) {
                        best_snap = Some((d, snapped));
                    }
                }
            }
        }
        for (i, other) in self.audio_clips.iter().enumerate() {
            if exclude == Some((DragTarget::Audio, i)) { continue; }
            for &edge in &[other.start_frame, other.end_frame()] {
                let d = (new_start - edge).abs();
                if d <= threshold && best_snap.map_or(true, |(bd, _)| d < bd) {
                    best_snap = Some((d, edge));
                }
                let d = (new_end - edge).abs();
                if d <= threshold {
                    let snapped = edge - frame_count as i64;
                    if snapped >= 0 && best_snap.map_or(true, |(bd, _)| d < bd) {
                        best_snap = Some((d, snapped));
                    }
                }
            }
        }

        if let Some((_, snapped)) = best_snap {
            new_start = snapped;
        }
        new_start
    }

    /// True if some other clip (on any track) covers the timeline frame
    /// `start_frame - 1` — i.e. the decoder will have produced output there,
    /// so this clip can safely drop its leading keyframe and pick up mid-GOP.
    /// Enables cross-track mosh: a V1 clip over V2 can mosh against V2 even
    /// without a V1 predecessor.
    pub fn has_mosh_predecessor(&self, idx: usize) -> bool {
        let start = self.clips[idx].start_frame;
        if start <= 0 {
            return false;
        }
        let target = start - 1;
        self.clips.iter().enumerate().any(|(j, c)| {
            j != idx && c.start_frame <= target && target < c.end_frame()
        })
    }

    /// Disable cross-clip mosh on any clip that no longer has a valid
    /// predecessor frame on either track, and restore its frame count.
    pub fn validate_mosh_state(&mut self) {
        for i in 0..self.clips.len() {
            if !self.clips[i].drop_leading_keyframe {
                continue;
            }
            if !self.has_mosh_predecessor(i) {
                self.clips[i].drop_leading_keyframe = false;
                self.clips[i].frame_count += 1;
            }
        }
    }
}
