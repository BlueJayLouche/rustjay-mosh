# Compression Artifacting

Where databending scrambles bytes arbitrarily, **compression artifacting** corrupts the image the way *real* compression does — by pushing a region through JPEG at brutally low quality. The result is authentic 8×8 **macroblocks**, DCT ringing, and chroma smearing, because it genuinely *is* DCT compression.

Open the **Compress** dialog from the Operations panel, set a region and quality, and [bake](README.md).

## The region

You define a rectangle over the frame:

- **x, y** — top-left corner of the region.
- **w, h** — its size. The defaults cover the full 1920×1080 frame; shrink it to artifact only part of the image.
- **quality** — the JPEG quality, **1–100**. Lower means more artifacts. Values in the low teens give the heavy blocky look; single digits obliterate the region into pure macroblocks.

The region is clamped to the frame, so a rectangle that runs off the right or bottom edge is honoured rather than rejected.

## What happens per frame

For each frame in the baked range, the selected region is:

1. Converted from YUV to RGB.
2. Encoded to JPEG at the chosen quality, then immediately decoded back.
3. Converted back to YUV and pasted into the frame.

Whatever the JPEG encoder threw away at that quality is now permanently part of the picture — real quantisation artifacts, not a simulation.

## Why this is the honest way to add blockiness

This is the *deliberate, controlled* cousin of the problem the [delivery presets](../exporting/delivery-presets.md) solve. Here you **want** macroblocks, so you dial them in with a known quality over a known region. Later, when you export, you *don't* want the platform adding its own uncontrolled blockiness on top — which is exactly what the delivery encode prevents. Same phenomenon, opposite intent.

## Tips

- **Animate intent with bakes.** Bake a low-quality region for a few frames to make compression "attack" and then clear.
- **Combine with mosh.** Artifact a clip, then mosh it — the blocky region bleeds along the motion vectors.
- **Localise it.** A small low-quality region over a face or logo, with the rest of the frame clean, draws the eye straight to the corruption.
