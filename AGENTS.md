# Agent Briefing — rustjay-mosh

> Last updated: 2026-04-20  
> Current commit: `main` branch, post-CI-and-bundling work

## What is this?

A Rust-based NLE (non-linear editor) for **datamoshing** — glitch-art video manipulation by dropping I-frames so P-frames decode against wrong/stale reference data.

- **Import** any video ffmpeg supports → transcode to long-GOP H.264 (1 I-frame + all P-frames)
- **Arrange** clips on a timeline, trim in/out points, snap-to-edge
- **Mosh** — drop a clip's leading keyframe so it bleeds into the previous clip
- **Render** to MP4 via direct packet remux (no re-encode)

## Build

```bash
cargo run --release
```

Needs: Rust 1.85+, FFmpeg 8.x, GPU with Metal/Vulkan/DX12 support.

## Architecture at a glance

```
┌─────────────────────────────────────────────────────────┐
│  ui::app (MoshApp) — eframe main loop, wires everything │
├──────────────────┬────────────────┬─────────────────────┤
│  importer        │  packet        │  preview::decoder   │
│  (ffmpeg CLI     │  (OwnedPacket  │  (flush + seek      │
│   transcode)     │   · PacketClip │   decode)           │
├──────────────────┴────────────────┴─────────────────────┤
│  ui::timeline_panel → ordered clips → flat packet seq   │
├──────────────────────────┬──────────────────────────────┤
│  ui::preview (wgpu)      │  render::muxer               │
│  YUV→RGB shader          │  remux to MP4                │
└──────────────────────────┴──────────────────────────────┘
         ↑                                    ↑
    audio::mod.rs                     audio mix → ffmpeg mux
```

## Key crates

| Crate | Role |
|---|---|
| `eframe` 0.29 + `wgpu` | UI window + GPU preview |
| `ffmpeg-next` 8 | Decode, mux, codec params |
| `hound` 3.5 | WAV export for audio mix |
| `rfd` 0.15 | Native file dialogs |
| `tempfile` 3 | Temp dirs for transcode/mux |
| `rayon` | (imported, not heavily used yet) |

## Module map (~2600 LOC total)

| Module | Lines | Purpose |
|---|---|---|
| `ui::app` | 700 | `MoshApp` — top bar, pool sidebar, controls sidebar, timeline panel, preview, render thread spawning |
| `ui::timeline_panel` | 780 | `TimelinePanel` widget — ruler, video lane, audio lane, drag/trim/snap, playhead, fade handles |
| `audio::mod` | 200 | `AudioClip`, `AudioTimelineClip`, `import_audio`, `render_audio_mix` with fades/crossfades |
| `importer::mod` | 130 | Transcode via ffmpeg CLI (`-g 99999999 -bf 0`), extract packets, decode first frame for preview |
| `preview::decoder` | 100 | `PacketDecoder` — flush + decode from nearest keyframe up to target frame |
| `render::muxer` | 60 | `export_packets` — remux `Vec<OwnedPacket>` to MP4 without re-encoding |
| `packet::mod` | 85 | `OwnedPacket`, `PacketClip`, `ClipSpan`, `build_sequence` |
| `codec::ir` | 35 | `Yuv420` struct |
| `datamosh::mod` | 40 | `remove_iframes`, `cross_clip_mosh` (legacy graph ops) |
| `frame_graph::mod` | 75 | DAG of frame references (legacy, kept for reference) |
| `format::binary` | 130 | Binary format structs (legacy from custom codec era) |

## Data flow

### Import (video)
1. User picks a file → `MoshApp::start_import` spawns thread
2. `importer::import_video` runs ffmpeg CLI: `ffmpeg -i input -vf scale=1280:720 -vcodec libx264 -g 99999999 -bf 0 -pix_fmt yuv420p temp.mp4`
3. Reads packets from temp MP4 into `Vec<OwnedPacket>`
4. Decodes first packet to get a `Yuv420` preview frame
5. Stores `PacketClip { packets, codec_parameters, time_base, ... }` in `MoshApp::packet_clips`

### Import (audio)
1. `audio::import_audio` runs ffmpeg CLI: `ffmpeg -i input -vn -ar 48000 -ac 2 -f f32le temp.raw`
2. Reads raw f32 samples, computes per-video-frame `Peak { min, max }`
3. Stores `AudioClip { samples, sample_rate, peaks, ... }` in `MoshApp::audio_clips`

### Timeline model
- **Video**: `TimelineClip { id, clip_idx, name, frame_count, source_frame_count, start_frame, source_offset, color, selected, drop_leading_keyframe }`
- **Audio**: `AudioTimelineClip { audio_clip_idx, start_frame, frame_count, source_offset, fade_in_frames, fade_out_frames, selected }`
- Clips live in `TimelinePanel::clips` / `::audio_clips`
- `source_offset` = head trim in frames (how many source packets to skip)
- `frame_count` = visible duration on timeline

### Preview
- `MoshApp::current_preview_yuv()` finds which timeline clip is under the playhead
- Calls `PacketDecoder::decode_up_to()` with the clip's packets
- Decoder finds nearest keyframe, flushes, decodes sequentially
- YUV frame is rendered via wgpu callback + BT.601 WGSL shader

### Render
1. Build `Vec<OwnedPacket>` from timeline clips in sorted order
2. Per clip: rebase PTS/DTS to be monotonic across the whole sequence (`pts_offset` accumulator)
3. If audio clips exist:
   - `export_packets()` → temp `video.mp4`
   - `audio::render_audio_mix()` → temp `audio.wav` (48kHz stereo f32)
   - ffmpeg CLI mux: `ffmpeg -y -i video.mp4 -i audio.wav -map 0:v:0 -map 1:a:0 -c:v copy -c:a aac -b:a 192k output.mp4`
4. If no audio: `export_packets()` directly to output path

## Critical gotchas / recent bugfixes

### Cross-clip mosh adjacency validation
`cross_clip_mosh()` sets `drop_leading_keyframe = true` and decrements `frame_count` by 1. If the user later **moves the clip away** from its predecessor, `can_drop` becomes false at render time but `frame_count` is still shrunk → the keyframe gets reinstated with wrong duration.

**Fix**: `TimelinePanel::validate_mosh_state()` runs after every drag release and after deletions. It checks if any `drop_leading_keyframe` clip is no longer immediately preceded by another clip (`end_frame() == start_frame`). If so, it disables mosh and restores `frame_count += 1`.

### ffmpeg mux stderr capture
Previously stderr was sent to `/dev/null`. Now captured so ffmpeg errors (missing streams, codec issues) surface in the UI status bar.

### Bundled ffmpeg binary
Release builds bundle the `ffmpeg` CLI binary inside the package (macOS: `Contents/MacOS/ffmpeg`; Windows: `ffmpeg.exe` next to the exe). `crate::bundled_ffmpeg()` in `lib.rs` resolves the binary path — it checks for a sibling `ffmpeg` next to the current executable first, falling back to PATH. **All three call sites must use `crate::bundled_ffmpeg()`**: `importer/mod.rs`, `audio/mod.rs`, `ui/app.rs`. Never add a fourth `Command::new("ffmpeg")` directly.

### Audio clip drag modes
Normal drag on audio clip edge = **trim** (same as video). Hold **Shift** + drag on left/right half = **fade in/out**.

### Packet iterator bug (fixed)
`ClipSpan::iter_packets()` was skipping the leading keyframe but not adjusting visible count, causing a 1-frame gap in renders. Fixed by shrinking `visible_count` when `drop_leading_keyframe` is true.

## Code conventions

- **No custom `AGENTS.md` in subdirs** — root `AGENTS.md` is the source of truth
- Rust edition 2024, stable toolchain
- `cargo check` before committing; warnings OK but avoid errors
- Minimal changes; match existing style (no semicolons on last expr, etc.)

## How to extend

| Want to… | Look at |
|---|---|
| Add a new mosh operation | `datamosh/mod.rs`, `ui/app.rs` right-panel buttons |
| Change timeline drag behavior | `ui/timeline_panel.rs` — `DragMode`, hit-test, drag logic |
| Add new render format | `render/muxer.rs`, `start_render()` in `ui/app.rs` |
| Change preview quality | `importer/mod.rs` transcode settings (CRF, preset) |
| Add audio effects | `audio/mod.rs` — `render_audio_mix()` is the mixing loop |
| Change zoom limits | `ui/timeline_panel.rs` — `clamp(0.5, 500.0)` |

## Contact / repo

- Repo: `github.com:BlueJayLouche/rustjay-mosh.git`
- Branch: `main`
- GitHub Actions release CI at `.github/workflows/release.yml` — triggers on `v*` tags, builds macOS ARM, Linux x86_64, Windows x86_64
