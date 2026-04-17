# rustjay-mosh

A pure-Rust datamoshing NLE (non-linear editor). Import video clips, place them on a timeline, rewire their I/P-frame reference graph to produce glitch-art motion-bleed effects, and render the result to MP4.

![status: early development](https://img.shields.io/badge/status-early%20development-orange)

---

## What is datamoshing?

Modern video codecs compress footage using two frame types:

- **I-frames** (intra) — fully self-contained images, like a JPEG.
- **P-frames** (predictive) — stored as *motion vectors* + small *residual deltas* relative to a prior frame.

Datamoshing exploits this structure deliberately:

| Technique | Effect |
|---|---|
| **I-frame removal** | Remove a keyframe so P-frames decode against stale content, causing pixels from a previous scene to "bleed" forward |
| **Cross-clip mosh** | Rewire a P-frame's reference to point at the last frame of a *different* clip; that clip's motion vectors now drive the other clip's pixels |

The result is the iconic "melting" or "smearing" glitch aesthetic found in music videos and experimental film.

---

## Features

- **FFmpeg importer** — open any format ffmpeg supports (mp4, mov, mkv, avi, webm, …)
- **Internal I/P codec** — re-encodes each clip as an I/P sequence (keyframe every 30 frames by default) with YUV420 planar pixel format, 16×16 macroblocks, and full Y+U+V residuals
- **Interactive timeline** — drag clips, scrub the playhead, zoom with Ctrl+scroll
- **I-frame markers** — red verticals on each clip block show every keyframe
- **Cross-clip mosh** — one click rewires the boundary P-frames of the selected clip to reference the previous clip's last frame
- **Interior I-frame removal** — bridges interior keyframes so motion flows continuously without resets
- **wgpu preview** — GPU-accelerated YUV→RGB display via a WGSL BT.601 shader; no CPU colour conversion
- **Render to MP4** — pipes raw YUV420 frames to `ffmpeg -c:v libx264` at a configurable frame rate

---

## Requirements

| Dependency | Version | Notes |
|---|---|---|
| Rust toolchain | 1.85+ | `rustup update stable` |
| FFmpeg | 8.x | `brew install ffmpeg` on macOS |
| A GPU with wgpu support | — | Metal (macOS), Vulkan, DX12 |

> **macOS**: eframe uses Metal via wgpu. No extra setup needed beyond Xcode command-line tools.

---

## Building

```sh
git clone https://github.com/BlueJayLouche/rustjay-mosh
cd rustjay-mosh
cargo run --release
```

A debug build works fine for short clips; use `--release` for longer ones (motion estimation is CPU-heavy).

---

## Usage

### Basic workflow

1. **Import clips** — click **➕ Import clip** (repeat for each clip).  
   Each clip is re-encoded into the internal I/P codec and placed end-to-end on the timeline.

2. **Arrange** — drag clips to reorder them on the timeline.  
   The playhead scrubs the preview.

3. **Cross-clip mosh** — select clip B, click **⚡ Cross-clip mosh**.  
   Clip B's first P-frame (and every GOP boundary) is rewired to reference clip A's last frame.  
   Clip A's pixels now morph through clip B's motion.

4. **Remove interior I-frames** *(optional)* — click **🗑 Remove interior I-frames** on the selected clip.  
   Interior keyframes are bridged over so the mosh effect doesn't reset every 30 frames.

5. **Render** — set the output FPS, click **🎬 Render to file…**, choose an output path.  
   Frames are decoded and piped to ffmpeg → H.264 MP4.

### Timeline controls

| Action | Gesture |
|---|---|
| Select clip | Click |
| Move clip | Drag |
| Move playhead | Click ruler or empty track |
| Pan timeline | Scroll |
| Zoom timeline | Ctrl + scroll |

---

## Architecture

```
┌─────────────────────────────────────────────────────────┐
│                      rustjay-mosh                       │
├──────────────────┬────────────────┬─────────────────────┤
│  importer        │  pool          │  codec              │
│  (ffmpeg-next 8) │  (MediaPool)   │  ir / encoder /     │
│                  │                │  decoder            │
├──────────────────┴────────────────┴─────────────────────┤
│  frame_store: Vec<Frame>  (flat I/P encoded sequence)   │
├─────────────────────────────────────────────────────────┤
│  datamosh engine                                        │
│  (cross_clip_mosh · remove_iframes · frame.reference    │
│   mutation — no separate graph traversal needed)        │
├──────────────────────────┬──────────────────────────────┤
│  ui::timeline_panel      │  ui::preview                 │
│  (egui painter widget)   │  (wgpu YUV callback)         │
├──────────────────────────┴──────────────────────────────┤
│  render (ffmpeg subprocess pipe → H.264 MP4)            │
└─────────────────────────────────────────────────────────┘
```

### Module map

| Path | Purpose |
|---|---|
| `codec::ir` | `Yuv420`, `Frame`, `MotionVector`, `Residual`, `MacroblockSize` |
| `codec::encoder` | I-frame encoder, block-matching P-frame encoder, `encode_clip_as_ip` |
| `codec::decoder` | Recursive P-frame decoder with full Y+U+V residual application |
| `format::binary` | `.mosh` v0.2 binary format — `FileHeader`, `FrameTableEntry`, `RawMotionVector` |
| `importer` | FFmpeg-based video importer → raw `Vec<Frame>` (all I-frames) |
| `pool` | `MediaPool` — stores imported assets by id |
| `frame_graph` | DAG of frame references (used for graph-level operations) |
| `datamosh` | `remove_iframes`, `cross_clip_mosh` graph operations |
| `render` | `export_video` → ffmpeg pipe, `decode_cached` |
| `ui::app` | `MoshApp` — eframe application, wires everything together |
| `ui::timeline_panel` | `TimelinePanel` egui widget — clips, I-frame markers, drag, playhead |
| `ui::preview` | `YuvResources` + `YuvPreviewCallback` — wgpu YUV→RGB render pipeline |
| `ui::shader.wgsl` | BT.601 YCbCr→RGB WGSL fragment shader |

---

## Internal codec

### Pixel format

YUV420 planar throughout the pipeline:

| Plane | Size |
|---|---|
| Y (luma) | `width × height` bytes |
| U (Cb) | `(width/2) × (height/2)` bytes |
| V (Cr) | `(width/2) × (height/2)` bytes |

Macroblocks: fixed **16×16** per clip (8×8 also supported).

### Residual layout

Each macroblock residual is stored as a flat `Vec<i16>`:

```
[Y: mb×mb][U: cmb×cmb][V: cmb×cmb]   where cmb = mb/2
```

For 16×16 blocks: 256 + 64 + 64 = 384 values per macroblock.

### Decoder algorithm

```
decode(frame, frame_store):
  if I-frame:
    return frame.planes
  ref = decode(frame_store[frame.reference], frame_store)
  for each macroblock (mv, residual):
    Y:  sample ref.y at (block + mv),        add Y residual, clamp [0,255]
    U:  sample ref.u at (block + mv/2),      add U residual, clamp [0,255]
    V:  sample ref.v at (block + mv/2),      add V residual, clamp [0,255]
  return output
```

Sampling clamps to edges (no wrap). `frame.reference` is mutated by mosh operations to create cross-clip prediction chains.

### Binary format (`.mosh` v0.2, little-endian)

```
[Header 16B][Asset metadata][Frame Table N×32B][Frame Blocks…]
```

**Header**: `MOSH` magic · u16 major · u16 minor · u64 frame_count  
**Frame table entry**: u64 offset · u64 size · u64 pts · u32 type · u32 reference  
**Frame block**: `[MVs][Residuals][Planes (I only)]`  
**Motion vector**: i16 dx · i16 dy · u16 bx · u16 by (8 bytes)

---

## Roadmap

- [ ] Audio passthrough in render
- [ ] GOP-size control per clip (currently fixed at 30)
- [ ] Motion vector visualisation overlay
- [ ] Waveform / thumbnail strip on timeline clips
- [ ] Selective I-frame removal (click individual markers to remove)
- [ ] Export to formats other than H.264

---

## License

MIT
