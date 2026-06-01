# Undo & Redo

Every timeline edit is undoable.

| Action | Shortcut | Button |
|---|---|---|
| Undo | `Ctrl+Z` | ↶ Undo |
| Redo | `Ctrl+Shift+Z` | ↷ Redo |

## What's covered

Undo/redo tracks **timeline arrangement** — the placements and edit decisions on the video and audio lanes:

- moving, trimming, and snapping clips
- placing a clip from the pool
- deleting clips
- applying or clearing a [cross-clip mosh](../moshing/README.md)
- placing a [baked](../glitch/README.md) clip onto the timeline

## How it works (and why it's cheap)

Undo is **snapshot-based**. At the end of any frame where the timeline changed — and only when you're *not* mid-drag, so a whole drag becomes one undo step — the previous arrangement is pushed onto an undo stack.

Crucially, a snapshot contains only the lightweight [timeline-clip placements](../core-concepts/clips-and-pool.md), never the media. The heavy `PacketClip` packets and audio samples live in the pool and aren't copied, so undo stays fast and memory-cheap even on a long session. The history holds up to 100 steps.

## What undo does *not* remove

Because the media pool is separate from timeline placements, undo restores **arrangement**, not your library:

- Undoing right after an **import** removes the clip from the *timeline*, but the source stays in the pool — drag it back out to use it again.
- Undoing after a **bake** removes the baked clip's timeline placement, but the baked source remains in the pool.

This is usually what you want: you can back out an arrangement mistake without re-importing or re-baking. If you genuinely want a source gone, that's a pool action, not an undo.

> Undo/redo history is per-session — it isn't stored in the saved `.rjmosh` bundle. Opening a project starts with a clean history.
