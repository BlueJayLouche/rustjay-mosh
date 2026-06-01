# Your First Mosh

We'll make a classic datamosh — two clips where the second one **bleeds into** the first — in about five minutes. The whole trick is dropping one keyframe.

## 1. Import two clips

Click **➕ Import clip** in the top bar and pick a video. Anything FFmpeg can read works; it's transcoded to a long-GOP H.264 stream (one keyframe, then all P-frames) and added to the **Clip Pool** on the left. Do it again for a second clip.

> Every clip is normalised to the project resolution (**1920×1080**) on import, so clips from different sources line up cleanly.

## 2. Drop them on the timeline

Drag each pool clip down onto the **video lane** at the bottom. Place the second clip **immediately after** the first so their edges touch — moshing only works when one clip directly follows another.

You now have a two-clip sequence. Drag the **playhead** along the ruler to scrub; the centre preview decodes the frame under the playhead live.

## 3. Mosh the cut

Select the **second** clip (click its body) and, in the **Operations** panel on the right, click **Cross-Clip Mosh**.

Here's what happens under the hood: the second clip's first frame is a keyframe — a complete, self-contained picture. Moshing **drops that keyframe**. Now, at the cut, the decoder keeps applying the second clip's *motion vectors* (the P-frames) on top of the *last picture of the first clip*. The motion is right; the picture underneath is wrong. The result is the signature smear — the first clip melting into the second's movement.

Scrub across the cut and watch the bleed.

## 4. Render it

Open the **Render** panel (right side), set your **FPS**, leave the preset on **Raw mosh**, and click **🎬 Render to file…**. The manipulated packet stream is remuxed straight to MP4 — no re-encode — so the file is a pixel-exact copy of what you previewed.

That's a complete datamosh. From here:

- Stack more clips and mosh several cuts.
- Trim clips so the bleed lands on a specific beat — see [The Timeline](../timeline/README.md).
- Bend or artifact a section with the [Bake Pipeline](../glitch/README.md).
- Before you post it anywhere, read [Exporting](../exporting/README.md) — Raw mosh looks great locally but platforms will re-compress it, and the delivery presets exist to fix exactly that.

Next: [A Tour of the Window](the-window.md).
