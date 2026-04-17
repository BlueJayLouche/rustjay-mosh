use crate::codec::ir::{Frame, FrameType, MotionVector, Yuv420};
use crate::format::binary::FormatError;

/// Decode a single frame, recursively resolving its reference chain.
pub fn decode_frame(frame: &Frame, frame_store: &[Frame]) -> Result<Yuv420, FormatError> {
    match frame.frame_type {
        FrameType::I => Ok(frame.planes.clone().expect("I-frame must carry planes")),
        FrameType::P => {
            let ref_idx = frame.reference.expect("P-frame must have a reference") as usize;
            let ref_planes = decode_frame(&frame_store[ref_idx], frame_store)?;
            let mut out = ref_planes.clone();
            apply_motion_and_residuals(frame, &ref_planes, &mut out);
            Ok(out)
        }
    }
}

fn apply_motion_and_residuals(frame: &Frame, reference: &Yuv420, out: &mut Yuv420) {
    let mb = frame.mb_size as i32;
    let cmb = mb / 2;
    let w = reference.width as i32;
    let h = reference.height as i32;
    let cw = reference.chroma_width() as i32;
    let ch = reference.chroma_height() as i32;

    let y_len = (mb * mb) as usize;
    let c_len = (cmb * cmb) as usize;

    for (mv, residual) in frame.motion_vectors.iter().zip(frame.residuals.iter()) {
        let d = &residual.data;
        let y_res = d.get(..y_len).unwrap_or(&[]);
        let u_res = d.get(y_len..y_len + c_len).unwrap_or(&[]);
        let v_res = d.get(y_len + c_len..y_len + 2 * c_len).unwrap_or(&[]);

        apply_block_luma(mv, mb, reference, out, w, h, y_res);
        apply_block_chroma(mv, cmb, reference, out, cw, ch, u_res, v_res);
    }
}

fn apply_block_luma(
    mv: &MotionVector,
    mb: i32,
    reference: &Yuv420,
    out: &mut Yuv420,
    w: i32,
    h: i32,
    residual: &[i16],
) {
    let base_x = mv.bx as i32 * mb;
    let base_y = mv.by as i32 * mb;

    for row in 0..mb {
        for col in 0..mb {
            let dst_x = base_x + col;
            let dst_y = base_y + row;
            if dst_x >= w || dst_y >= h {
                continue;
            }
            let src_x = (base_x + col + mv.dx as i32).clamp(0, w - 1);
            let src_y = (base_y + row + mv.dy as i32).clamp(0, h - 1);
            let res_idx = (row * mb + col) as usize;
            let delta = residual.get(res_idx).copied().unwrap_or(0);
            let val = reference.y[(src_y * w + src_x) as usize] as i16 + delta;
            out.y[(dst_y * w + dst_x) as usize] = val.clamp(0, 255) as u8;
        }
    }
}

fn apply_block_chroma(
    mv: &MotionVector,
    cmb: i32,
    reference: &Yuv420,
    out: &mut Yuv420,
    cw: i32,
    ch: i32,
    u_res: &[i16],
    v_res: &[i16],
) {
    let cdx = mv.dx as i32 / 2;
    let cdy = mv.dy as i32 / 2;
    let cbx = mv.bx as i32 * cmb;
    let cby = mv.by as i32 * cmb;

    for row in 0..cmb {
        for col in 0..cmb {
            let dst_x = cbx + col;
            let dst_y = cby + row;
            if dst_x >= cw || dst_y >= ch {
                continue;
            }
            let src_x = (cbx + col + cdx).clamp(0, cw - 1);
            let src_y = (cby + row + cdy).clamp(0, ch - 1);
            let src_idx = (src_y * cw + src_x) as usize;
            let dst_idx = (dst_y * cw + dst_x) as usize;
            let res_idx = (row * cmb + col) as usize;

            let u_delta = u_res.get(res_idx).copied().unwrap_or(0);
            let v_delta = v_res.get(res_idx).copied().unwrap_or(0);
            out.u[dst_idx] = (reference.u[src_idx] as i16 + u_delta).clamp(0, 255) as u8;
            out.v[dst_idx] = (reference.v[src_idx] as i16 + v_delta).clamp(0, 255) as u8;
        }
    }
}
