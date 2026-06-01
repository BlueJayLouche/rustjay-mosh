# Clips & the Pool

There are two distinct ideas the rest of the guide leans on: the **pool clip** (a source) and the **timeline clip** (a placement). Keeping them separate explains a lot of the app's behaviour.

## Pool clips are sources

The **Clip Pool** holds your imported material — one `PacketClip` per video source, one `AudioClip` per audio source. These are the heavy objects: they own the actual encoded packets (video) or sample data (audio).

A pool clip is a *library item*. Placing it on the timeline does not remove or alter it.

## Timeline clips are placements

A clip on the timeline is a lightweight **reference** into a pool clip:

```rust
pub struct TimelineClip {
    pub clip_idx: usize,           // which pool clip this points at
    pub start_frame: i64,          // where it sits on the timeline
    pub frame_count: usize,        // visible length after trimming
    pub source_offset: usize,      // frames trimmed off the head
    pub drop_leading_keyframe: bool, // moshed?
    pub track: u8,                 // 0 = top, 1 = bottom
    // … id, name, colour, selection
}
```

Because it's just a reference plus edit decisions, you can:

- Place the **same** pool clip on the timeline many times.
- Trim each placement differently.
- Mosh one placement and leave another clean.

This is also why **undo** is cheap — it snapshots these tiny placement structs, never the media (see [Undo & Redo](../projects/undo.md)).

## Baked clips are new sources

Most timeline clips point at imported pool clips. The exception is **baking**: when you apply a databend or compression effect to a region, the result is encoded into a brand-new `PacketClip` that's added to the pool, and a timeline clip is placed over the region pointing at it. Baked clips behave like any other source — you can mosh them, trim them, and place them again. See [The Bake Pipeline](../glitch/README.md).

> Because baked clips are generated in-app and have no original file on disk, the project format has to **embed** all media rather than reference it — that's the whole reason `.rjmosh` is a self-contained bundle. See [Saving & Sharing](../projects/README.md).

## Two video tracks

The video lane has two tracks. **Track 0 is on top** and wins when clips overlap in time — handy for layering a moshed take over a clean one. Track 1 is the default landing track for newly imported clips.
