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
- **Auto audio extraction** — importing a video file automatically extracts its embedded audio track and places it on the audio lane, frame-aligned with the video clip.
- **Packet-based datamoshing** — no custom software codec. We manipulate the raw H.264 packet stream directly, so output looks identical to professional tools like Supermosh.
- **Fast import** — clips appear on the timeline instantly; no background P-frame encoding step.
- **Clip pool** — imported clips live in a left sidebar; drag any pool item onto the timeline (video or audio).
- **Interactive timeline** — drag clips, scrub the playhead, zoom with Ctrl+scroll, timecode ruler (`hh:mm:ss:ff`)
- **Trim handles** — drag the left/right edges of a placed clip to set in/out points; audio trim handles snap to video clip edges
- **Snap-to-edge** — dragging or trimming a clip snaps its moving edge to the nearest clip edge
- **Top-clip selection** — clicking where clips overlap always selects the topmost (visually frontmost) clip
- **Non-blocking preview** — playhead scrubbing and ruler selection update instantly; preview frames decode in a background thread so the UI never stalls
- **Cross-clip mosh** — one click drops the leading keyframe of the selected clip so it bleeds into the preceding clip
- **Glitch effects (Bake)** — decode → manipulate pixels → re-encode. Includes data bending (reverse scanlines, echo, bitcrush, byte swap, XOR, noise) and JPEG compression artifacting on arbitrary regions.
- **Audio tracks** — import audio (or extract from video), place on timeline, trim, fade in/out, crossfade. Renders to AAC 192 kbps.
- **Multi-track video** — stack clips on two video tracks (0 = top, 1 = bottom)
- **wgpu preview** — GPU-accelerated YUV→RGB display via a WGSL BT.601 shader; no CPU colour conversion
- **Render to MP4** — remuxes the manipulated packet stream directly to H.264 MP4 without re-encoding, with mixed audio

---

## Requirements

| Dependency | Version | Notes |
|---|---|---|
| Rust toolchain | 1.85+ | `rustup update stable` |
| FFmpeg | 8.x | `brew install ffmpeg` on macOS |
| A GPU with wgpu support | — | Metal (macOS), Vulkan, DX12 |

> **macOS**: eframe uses Metal via wgpu. No extra setup needed beyond Xcode command-line tools.

---

## Download

Pre-built binaries for macOS (ARM + Intel), Linux, and Windows are available on the [Releases](https://github.com/BlueJayLouche/rustjay-mosh/releases) page.

> **Linux** — the binary dynamically links against FFmpeg. Install it first: `sudo apt install ffmpeg` (Debian/Ubuntu) or the equivalent for your distro.

> **macOS Gatekeeper warning** — because the app is not notarized, macOS will block it on first launch. After downloading and installing, run this once in Terminal:
> ```sh
> xattr -cr "/Applications/RustJay Mosh.app"
> ```
> Then open normally. Alternatively, go to **System Settings → Privacy & Security** and click **Open Anyway** after the first blocked attempt.

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
   If the file contains an audio track it is automatically extracted and placed on the audio lane, aligned with the video clip.

2. **Build the timeline** — drag clips from the pool onto the timeline track, or rearrange existing clips by dragging their bodies. Clips snap end-to-end automatically when dragged close to another edge.

3. **Trim** — drag the left or right edge of a placed clip to trim its in/out points. A red vertical line shows the leading keyframe when it is still visible; a dark stripe indicates trimmed-away head frames.

4. **Cross-clip mosh** — select clip B, click **⚡ Cross-clip mosh**.  
   Clip B's leading I-frame is dropped and the clip shrinks by one frame; its P-frames now decode against clip A's pixels.  
   Clip A's pixels morph through clip B's motion.

5. **Glitch effects (Bake)** — select a clip, open **🌀 Data Bend** or **🗜 Compress Region**, adjust parameters, and click **Bake**.  
   The segment is decoded, the effect is applied per-frame, and the result is re-encoded to a new H.264 clip that replaces the selection.  
   Effects include reverse scanlines, echo, bitcrush, byte swap, XOR, noise, and low-quality JPEG re-compression regions.

6. **Audio** — audio is extracted automatically on video import, or drag a standalone audio file into the audio lane.  
   Trim the edges of an audio clip to snap them to video clip boundaries.  
   Hold **Shift** and drag the **left half** of an audio clip rightward to adjust fade in; hold **Shift** and drag the **right half** leftward to adjust fade out. Crossfades are automatic when adjacent clips have overlapping fade regions.

7. **Render** — set the output FPS, click **🎬 Render to file…**, choose an output path.  
   The packet sequence is rewritten with monotonic timestamps and remuxed to MP4. If audio clips exist, a 48 kHz stereo mix is rendered and muxed to AAC.

### Timeline controls

| Action | Gesture |
|---|---|
| Select clip | Click body (topmost clip wins when stacked) |
| Move clip | Drag body |
| Trim in | Drag left edge (snaps to clip edges) |
| Trim out | Drag right edge (snaps to clip edges) |
| Fade in (audio) | Shift + drag right on left half |
| Fade out (audio) | Shift + drag left on right half |
| Move playhead | Click ruler or empty track |
| Select range | Drag on ruler |
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
| `audio` | `AudioClip`, `AudioTimelineClip`, `import_audio`, `render_audio_mix` |
| `bake` | `bake_segment` — decode, apply effects (bend/compress), re-encode to H.264 |
| `frame_graph` | DAG of frame references (legacy data structure) |
| `datamosh` | Graph-level operations (legacy, kept for reference) |
| `ui::app` | `MoshApp` — eframe application, wires everything together |
| `ui::timeline_panel` | `TimelinePanel` egui widget — clips, drag, trim handles, snap, playhead, audio lane |
| `ui::preview` | `YuvResources` + `YuvPreviewCallback` — wgpu YUV→RGB render pipeline |
| `ui::shader.wgsl` | BT.601 YCbCr→RGB WGSL fragment shader |

---

## Roadmap

- [x] Timecode ruler (`hh:mm:ss:ff`) on timeline
- [x] Audio track support with fades and visible waveforms
- [x] Audio passthrough in render
- [x] Selective frame dropping / duplicating for advanced glitch effects (bake pipeline)
- [x] Auto audio extraction on video import
- [x] Non-blocking preview — scrubbing and selection never block the UI
- [x] Audio trim snapping to video clip edges
- [ ] Motion vector visualisation overlay
- [ ] Thumbnail strip on timeline clips
- [ ] Export to formats other than H.264

---

## License

MIT
