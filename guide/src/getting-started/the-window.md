# A Tour of the Window

rustjay-mosh is a single window with five regions.

```
┌──────────────────────────────────────────────────────────────┐
│ Top bar — Project menu · Import · Undo/Redo · status          │
├────────────┬────────────────────────────────────┬────────────┤
│            │                                     │            │
│ Clip Pool  │            Preview                  │ Operations │
│ (left)     │         (GPU, centre)              │  + Render  │
│            │                                     │  (right)   │
│            │                                     │            │
├────────────┴────────────────────────────────────┴────────────┤
│ Timeline — ruler · video lane · audio lane (bottom)           │
└──────────────────────────────────────────────────────────────┘
```

## Top bar

- **📁 Project** — Save / Open, Recent projects, *Collect files to share*, and *Export for all platforms*. See [Projects](../projects/README.md).
- **➕ Import clip** — bring in video or audio.
- **↶ Undo / ↷ Redo** — timeline edit history (`Ctrl+Z` / `Ctrl+Shift+Z`).
- A **`[project *]`** tag (the `*` means unsaved changes) and the live **status line**.

## Clip Pool (left)

Every imported source lives here — **Video** clips and **Audio** clips, each showing its length in frames. Drag a pool item onto the timeline to place it. The pool is your library; placing a clip doesn't consume it, so one source can appear many times on the timeline.

## Preview (centre)

A GPU-rendered view of the frame under the playhead. The decode runs on a background thread and a BT.601 WGSL shader converts YUV→RGB, so scrubbing stays smooth. When a clip is moshed, the preview shows the *actual* decoded result — bleed and all.

## Operations + Render (right)

Context-sensitive actions for the selected clip:

- **Cross-Clip Mosh** — drop the selected clip's leading keyframe (see [Moshing](../moshing/README.md)).
- **Databend… / Compress…** — open the glitch dialogs, then **Bake** the effect into a new clip (see [Glitch Effects](../glitch/README.md)).
- **Render** — output FPS, the **export preset** selector, and **🎬 Render to file…** (see [Exporting](../exporting/README.md)).
- **Timeline** — zoom controls and selection tools.

## Timeline (bottom)

The arrangement surface: a **ruler** (drag to scrub or to select a range), a **video lane** (two tracks — 0 is on top and wins when clips stack), and an **audio lane**. This is where you trim, move, snap, mosh, and select regions to bake. Full details in [The Timeline](../timeline/README.md).
