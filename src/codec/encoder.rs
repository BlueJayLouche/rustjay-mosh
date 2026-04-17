use std::ops::Range;

use crate::codec::ir::{Frame, FrameType, MacroblockSize, MotionVector, Residual, Yuv420};

/// Encode a Yuv420 frame as an I-frame (no prediction).
pub fn encode_iframe(planes: Yuv420, pts: u64, mb_size: MacroblockSize) -> Frame {
    Frame {
        frame_type: FrameType::I,
        pts,
        mb_size,
        reference: None,
        planes: Some(planes),
        motion_vectors: vec![],
        residuals: vec![],
    }
}

/// Encode a P-frame by computing motion vectors and Y+U+V residuals relative to `reference`.
///
/// Residual layout per macroblock: `[Y: mb²][U: cmb²][V: cmb²]` where `cmb = mb/2`.
/// Uses exhaustive block-matching luma SAD within `search_range` pixels.
pub fn encode_pframe(
    current: &Yuv420,
    reference: &Yuv420,
    ref_idx: u32,
    pts: u64,
    mb_size: MacroblockSize,
    search_range: i16,
) -> Frame {
    let mb = mb_size as i32;
    let w = current.width as i32;
    let h = current.height as i32;
    let cols = (w + mb - 1) / mb;
    let rows = (h + mb - 1) / mb;

    let mut motion_vectors = Vec::with_capacity((cols * rows) as usize);
    let mut residuals = Vec::with_capacity((cols * rows) as usize);

    for by in 0..rows {
        for bx in 0..cols {
            let mv = find_best_mv(current, reference, bx, by, mb, w, h, search_range);
            let res = compute_residual(current, reference, &mv, mb, w, h);
            motion_vectors.push(mv);
            residuals.push(res);
        }
    }

    Frame {
        frame_type: FrameType::P,
        pts,
        mb_size,
        reference: Some(ref_idx),
        planes: None,
        motion_vectors,
        residuals,
    }
}

/// Encode raw decoded frames into an interleaved I/P sequence and append them
/// to `store`. Returns the slice of `store` that was added.
///
/// I-frames are inserted at position 0 and every `keyframe_interval` frames.
/// Each P-frame references the immediately preceding frame in `store`.
/// `search_range` controls the half-range (in pixels) for motion estimation.
pub fn encode_clip_as_ip(
    raw_frames: &[Yuv420],
    mb_size: MacroblockSize,
    search_range: i16,
    keyframe_interval: usize,
    pts_offset: u64,
    store: &mut Vec<Frame>,
) -> Range<usize> {
    let start = store.len();

    for (i, yuv) in raw_frames.iter().enumerate() {
        let pts = pts_offset + i as u64;
        let is_keyframe = i == 0 || keyframe_interval > 0 && i % keyframe_interval == 0;

        if is_keyframe {
            store.push(encode_iframe(yuv.clone(), pts, mb_size));
        } else {
            let ref_raw = &raw_frames[i - 1];
            let ref_store_idx = (store.len() - 1) as u32;
            store.push(encode_pframe(yuv, ref_raw, ref_store_idx, pts, mb_size, search_range));
        }
    }

    start..store.len()
}

fn find_best_mv(
    current: &Yuv420,
    reference: &Yuv420,
    bx: i32,
    by: i32,
    mb: i32,
    w: i32,
    h: i32,
    search_range: i16,
) -> MotionVector {
    let sr = search_range as i32;
    let mut best_sad = u64::MAX;
    let mut best_dx = 0i16;
    let mut best_dy = 0i16;

    for dy in -sr..=sr {
        for dx in -sr..=sr {
            let sad = block_sad(current, reference, bx, by, mb, w, h, dx, dy);
            if sad < best_sad {
                best_sad = sad;
                best_dx = dx as i16;
                best_dy = dy as i16;
            }
        }
    }

    MotionVector { dx: best_dx, dy: best_dy, bx: bx as u16, by: by as u16 }
}

fn block_sad(
    current: &Yuv420,
    reference: &Yuv420,
    bx: i32,
    by: i32,
    mb: i32,
    w: i32,
    h: i32,
    dx: i32,
    dy: i32,
) -> u64 {
    let mut sad = 0u64;
    for row in 0..mb {
        for col in 0..mb {
            let cx = bx * mb + col;
            let cy = by * mb + row;
            if cx >= w || cy >= h {
                continue;
            }
            let rx = (cx + dx).clamp(0, w - 1);
            let ry = (cy + dy).clamp(0, h - 1);
            let diff = current.y[(cy * w + cx) as usize] as i32
                - reference.y[(ry * w + rx) as usize] as i32;
            sad += diff.unsigned_abs() as u64;
        }
    }
    sad
}

/// Residual layout: [Y: mb*mb][U: cmb*cmb][V: cmb*cmb] where cmb = mb/2.
fn compute_residual(
    current: &Yuv420,
    reference: &Yuv420,
    mv: &MotionVector,
    mb: i32,
    w: i32,
    h: i32,
) -> Residual {
    let cmb = mb / 2;
    let cw = w / 2;
    let ch = h / 2;
    let cdx = mv.dx as i32 / 2;
    let cdy = mv.dy as i32 / 2;
    let cbx = mv.bx as i32 * cmb;
    let cby = mv.by as i32 * cmb;

    let cap = (mb * mb + 2 * cmb * cmb) as usize;
    let mut data = Vec::with_capacity(cap);

    // Y
    for row in 0..mb {
        for col in 0..mb {
            let cx = mv.bx as i32 * mb + col;
            let cy = mv.by as i32 * mb + row;
            if cx >= w || cy >= h {
                data.push(0i16);
                continue;
            }
            let rx = (cx + mv.dx as i32).clamp(0, w - 1);
            let ry = (cy + mv.dy as i32).clamp(0, h - 1);
            data.push(current.y[(cy * w + cx) as usize] as i16
                - reference.y[(ry * w + rx) as usize] as i16);
        }
    }

    // U then V
    for plane in [
        (current.u.as_slice(), reference.u.as_slice()),
        (current.v.as_slice(), reference.v.as_slice()),
    ] {
        for row in 0..cmb {
            for col in 0..cmb {
                let cx = cbx + col;
                let cy = cby + row;
                if cx >= cw || cy >= ch {
                    data.push(0i16);
                    continue;
                }
                let rx = (cx + cdx).clamp(0, cw - 1);
                let ry = (cy + cdy).clamp(0, ch - 1);
                data.push(plane.0[(cy * cw + cx) as usize] as i16
                    - plane.1[(ry * cw + rx) as usize] as i16);
            }
        }
    }

    Residual { data }
}
