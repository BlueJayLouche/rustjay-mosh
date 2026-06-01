# The Bake Pipeline

Moshing breaks video at the **frame-reference** level. The bake pipeline breaks it at the **pixel** level — databending and compression artifacts — and then folds the result back into the timeline as a new, fully moshable clip.

## Why "bake"?

The editor's normal currency is encoded packets, which it never decodes for editing. But databending and JPEG artifacting operate on actual pixels. So these effects can't just be a flag — they have to be **rendered into new frames**. That render-and-re-encode step is the *bake*.

## The four stages

When you bake a databend or compression effect over a selected range, the app runs a background job:

1. **Decode** just the selected range of the selected clip to raw `YUV420` frames (seeking to the nearest keyframe first).
2. **Apply** the chosen effect to every decoded frame — a [databend](databending.md) mode or a [compression](compression.md) region.
3. **Re-encode** the processed frames to H.264 with **one keyframe + all P-frames** (`gop = max`, `bf = 0`, CRF 18) — the same long-GOP layout as an imported clip, so the baked clip is itself moshable.
4. **Read back** the encoded packets into a new `PacketClip`, add it to the pool, and place a timeline clip over the original region.

A progress bar tracks the job, and you can **cancel** mid-bake — the job checks for cancellation between stages and per frame, and produces no output if you stop it.

## Choosing what to bake

- **Select a range** on the ruler (see [Arranging Clips](../timeline/README.md)) to set how much to bake. The bake applies to that range intersected with the selected clip.
- Bake durations can be a single frame, a few frames around the playhead, or a whole clip — short bakes are great for a one-frame "hit," longer ones for a sustained corruption.

## Baked clips are first-class

Because the baked output is a real long-GOP `PacketClip`, you can do everything to it that you can do to an import:

- **Mosh it** against the clip before it.
- **Trim** and re-place it.
- **Bake again** on top for stacked corruption.

The two effect families are covered next: [Databending](databending.md) and [Compression Artifacting](compression.md).
