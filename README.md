# rustjay-mosh

A pure-Rust datamoshing NLE (non-linear editor). Import video clips, place them on a timeline, drop keyframes to produce glitch-art motion-bleed effects, and render the result to MP4 — all using native H.264 packet manipulation, no slow custom encoder.

![status: early development](https://img.shields.io/badge/status-early%20development-orange)

---

## What is datamoshing?

Modern video codecs compress footage using two frame types:

- **I-frames** (intra) — fully self-contained images, like a JPEG.
- **P-frames** (predictive) — stored as *motion vectors* + small *residual deltas* relative to a prior frame.

Datamoshing exploits this structure deliberately:

| Technique | Effect |
|---|---|
| **I-frame removal** | Drop a keyframe so P-frames decode against stale content, causing pixels from a previous scene to "bleed" forward |
| **Cross-clip mosh** | Concatenate two clips and skip the leading keyframe of the second clip; its P-frames now drive the first clip's pixels |

The result is the iconic "melting" or "smearing" glitch aesthetic found in music videos and experimental film.

---

## Features

- **FFmpeg importer** — open any format ffmpeg supports (mp4, mov, mkv, avi, webm, …). Each clip is transparently transcoded to a long-GOP H.264 intermediate with one I-frame and all P-frames.
- **Packet-based datamoshing** — no custom software codec. We manipulate the raw H.264 packet stream directly, so output looks identical to professional tools like Supermosh.
- **Fast import** — clips appear on the timeline instantly; no background P-frame encoding step.
- **Clip pool** — imported clips live in a left sidebar; drag any pool item onto the timeline
- **Interactive timeline** — drag clips, scrub the playhead, zoom with Ctrl+scroll
- **Trim handles** — drag the left/right edges of a placed clip to set in/out points
- **Snap-to-edge** — dragging a clip body snaps its start or end to the nearest clip edge
- **Cross-clip mosh** — one click drops the leading keyframe of the selected clip so it bleeds into the preceding clip
- **wgpu preview** — GPU-accelerated YUV→RGB display via a WGSL BT.601 shader; no CPU colour conversion
- **Render to MP4** — remuxes the manipulated packet stream directly to H.264 MP4 without re-encoding

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

---

## Usage

### Basic workflow

1. **Import clips** — click **➕ Import clip** (repeat for each clip).  
   Each clip is transcoded to a one-keyframe H.264 stream and appears in the **Clip Pool** on the left.

2. **Build the timeline** — drag clips from the pool onto the timeline track, or rearrange existing clips by dragging their bodies. Clips snap end-to-end automatically when dragged close to another edge.

3. **Trim** — drag the left or right edge of a placed clip to trim its in/out points. A red vertical line shows the leading keyframe when it is still visible; a dark stripe indicates trimmed-away head frames.

4. **Cross-clip mosh** — select clip B, click **⚡ Cross-clip mosh**.  
   Clip B's leading I-frame is dropped and the clip shrinks by one frame; its P-frames now decode against clip A's pixels.  
   Clip A's pixels morph through clip B's motion.

5. **Render** — set the output FPS, click **🎬 Render to file…**, choose an output path.  
   The packet sequence is rewritten with monotonic timestamps and remuxed to MP4.

### Timeline controls

| Action | Gesture |
|---|---|
| Select clip | Click body |
| Move clip | Drag body |
| Trim in/out | Drag left/right edge |
| Move playhead | Click ruler or empty track |
| Pan timeline | Scroll |
| Zoom timeline | Ctrl + scroll |

---

## Architecture

```
┌─────────────────────────────────────────────────────────┐
│                      rustjay-mosh                       │
├──────────────────┬────────────────┬─────────────────────┤
│  importer        │  packet        │  preview::decoder   │
│  (ffmpeg CLI +   │  (OwnedPacket  │  (flush + seek      │
│   ffmpeg-next)   │   · PacketClip │   decode)           │
├──────────────────┴────────────────┴─────────────────────┤
│  timeline_panel → ordered ClipSpans → flat packet seq   │
├──────────────────────────┬──────────────────────────────┤
│  ui::preview             │  render::muxer               │
│  (wgpu YUV callback)     │  (ffmpeg remux → MP4)        │
└──────────────────────────┴──────────────────────────────┘
```

### How it works

1. **Import** runs `ffmpeg -g 99999999 -bf 0` to create an intermediate MP4 where only frame 0 is an I-frame.
2. We read the encoded H.264 packets from that file and store them as `OwnedPacket` inside a `PacketClip`.
3. The timeline builds a `ClipSpan` for each visible clip. If `drop_leading_keyframe` is true, the span skips the first packet (the I-frame).
4. **Preview** flushes the ffmpeg decoder, feeds packets from the last keyframe up to the playhead, and returns the final decoded YUV frame.
5. **Render** flattens all spans into a contiguous `Vec<OwnedPacket>`, rewrites PTS/DTS offsets so they are monotonic, and remuxes directly to MP4 with `av_interleaved_write_frame`.

### Module map

| Path | Purpose |
|---|---|
| `packet` | `OwnedPacket`, `PacketClip`, `ClipSpan`, `build_sequence` |
| `preview::decoder` | `PacketDecoder` — flush + sequential decode up to any frame |
| `render::muxer` | `export_packets` — remux packet slice to MP4 without re-encoding |
| `importer` | FFmpeg transcode + packet extraction |
| `frame_graph` | DAG of frame references (legacy data structure) |
| `datamosh` | Graph-level operations (legacy, kept for reference) |
| `ui::app` | `MoshApp` — eframe application, wires everything together |
| `ui::timeline_panel` | `TimelinePanel` egui widget — clips, drag, trim handles, snap, playhead |
| `ui::preview` | `YuvResources` + `YuvPreviewCallback` — wgpu YUV→RGB render pipeline |
| `ui::shader.wgsl` | BT.601 YCbCr→RGB WGSL fragment shader |

---

## Roadmap

- [ ] Timecode ruler (`hh:mm:ss:ff`) on timeline
- [ ] Audio track support with fades and visible waveforms
- [ ] Audio passthrough in render
- [ ] Motion vector visualisation overlay
- [ ] Thumbnail strip on timeline clips
- [ ] Selective frame dropping / duplicating for advanced glitch effects
- [ ] Export to formats other than H.264

---

## License

MIT
