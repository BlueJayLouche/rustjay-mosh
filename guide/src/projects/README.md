# Saving & Sharing

A project saves everything about your edit so you can close the app and pick up later — or hand the whole thing to someone else.

## The `.rjmosh` bundle

A saved project is **not** a single small file pointing at media elsewhere. It's a **self-contained bundle directory**:

```
my_project.rjmosh/
  project.json            the manifest — timeline edits, FPS, export preset, zoom, playhead
  media/clip_0.mp4        one file per video pool clip
  audio/audio_0.wav       one file per audio pool clip
```

The bundle **embeds all the media**. There's a concrete reason for that: [baked clips](../glitch/README.md) are generated inside the app and have no original file on disk to relink to. A project that only stored file paths could never reload a baked glitch. So rustjay-mosh saves the actual clips — video as remuxed MP4s (no re-encode, so they're byte-for-byte your packets), audio as WAVs — and reconstructs everything on load.

## Save & open

| Action | Where | Shortcut |
|---|---|---|
| Save | 📁 Project → Save | `Ctrl+S` |
| Save As | 📁 Project → Save As… | |
| Open | 📁 Project → Open… | `Ctrl+O` |
| Recent | 📁 Project → Recent projects | |

The top bar shows the project name with a `*` while there are unsaved changes. **Open** points at a `.rjmosh` folder; the app reads the manifest, loads every embedded clip back into the pool, rebuilds the timeline, and restores your FPS, zoom, playhead, and export-preset choice.

> The most-recently-used projects (up to 10) are remembered between sessions and listed under **Recent projects**.

## Collect files to share

**📁 Project → Collect files to share (.zip)…** packs the entire bundle into a single `.zip` for sending to a collaborator. It builds a fresh bundle into a temp location and zips it, so it works **even if you've never saved the project** — a one-step "give me one file I can send."

The recipient unzips it and opens the `.rjmosh` folder; because everything is embedded, it opens identically on their machine — baked clips and all.

## Related

- [Autosave & Recovery](autosave.md) — so a crash doesn't cost you work.
- [Undo & Redo](undo.md) — timeline edit history.
- [Export for All Platforms](../exporting/export-all.md) — for delivering finished video, as opposed to saving an editable project.
