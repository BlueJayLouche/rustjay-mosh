# Keyboard Shortcuts

## Project & editing

| Shortcut | Action |
|---|---|
| `Ctrl+N` | New project (confirms if there are unsaved changes) |
| `Ctrl+S` | Save project (prompts for a location the first time) |
| `Ctrl+O` | Open a `.rjmosh` project |
| `Ctrl+Z` | Undo |
| `Ctrl+Shift+Z` | Redo |
| `Delete` | Delete selected clip(s) |

> On macOS, `Cmd` stands in for `Ctrl`.

## Timeline gestures

| Action | Gesture |
|---|---|
| Select clip | Click body (topmost clip wins when stacked) |
| Move clip | Drag body |
| Trim in | Drag left edge (snaps to clip edges) |
| Trim out | Drag right edge (snaps to clip edges) |
| Fade in (audio) | `Shift` + drag right on the clip's left half |
| Fade out (audio) | `Shift` + drag left on the clip's right half |
| Move playhead | Click the ruler or empty track |
| Select a range | Drag on the ruler |
| Pan timeline | Scroll |
| Zoom timeline | `Ctrl` + scroll |

## Export presets

| Preset | Canvas | Notes |
|---|---|---|
| Raw mosh | source | Direct remux, pixel-exact, no re-encode |
| Reels — Blur BG | 1080×1920 | Blurred-background fill, whole frame kept |
| Reels — Crop | 1080×1920 | Centre crop |
| Reels — Triptych | 1080×1920 | Three stacked copies |
| Feed — Square | 1080×1080 | Centre-cropped square |
| Feed — Landscape | 1080×608 | Fit 16:9 |
| YouTube 1080p | 1920×1080 | High-bitrate master |
| YouTube 4K | 3840×2160 | Anti-compression upscale |

## Project file locations

| What | Path |
|---|---|
| Project bundle | a `*.rjmosh` directory you choose |
| Autosave recovery | `<config>/rustjay-mosh/autosave/recovery.rjmosh` |
| Recent projects | `<config>/rustjay-mosh/recent.json` |

## Key defaults

| Setting | Value |
|---|---|
| Project resolution | 1920×1080 |
| Import transcode | `libx264`, `-g 99999999 -bf 0`, CRF 18 |
| Audio | 48 kHz stereo, AAC out (256k / 384k for YouTube) |
| Undo history | up to 100 steps |
| Autosave interval | ~120 s while dirty and idle |

## See also

- The repository [`README.md`](https://github.com/BlueJayLouche/rustjay-mosh) for build and contribution notes.
- `AGENTS.md` in the repo for an architecture-level briefing.
