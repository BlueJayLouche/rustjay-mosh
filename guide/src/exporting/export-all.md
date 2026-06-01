# Export for All Platforms

When a mosh is done, you usually want to post it *everywhere* — a vertical Reel, a square Feed post, a YouTube upload. Rendering each one by one is tedious. **Export for all platforms** does them in a single action.

## Using it

**📁 Project → Export for all platforms…**, then pick a destination folder. rustjay-mosh renders the timeline through a curated set of presets and drops the results in that folder:

```
mosh_reels_blur.mp4      Instagram Reels (9:16, blurred background)
mosh_feed_square.mp4     Instagram Feed (1:1)
mosh_youtube_1080.mp4    YouTube (1920×1080)
mosh_youtube_4k.mp4      YouTube (3840×2160, anti-compression upscale)
```

Each file is a full [delivery master](delivery-presets.md) for that target — correct canvas, bitrate, GOP, colour, and faststart.

## How it stays efficient

The expensive part of a render is flattening the timeline and muxing the moshed video; encoding each preset is comparatively cheap. So the batch does the shared work **once**:

1. The moshed packet sequence is remuxed to a single temporary `video.mp4`.
2. The audio lane is mixed to a single `audio.wav`.
3. Then each preset's encode runs in turn against those shared inputs, writing its own output file.

The status line names each preset as it goes (*"Exporting Instagram Reels — Blur BG…"*), and the work runs on a background thread so the app stays responsive.

## Tip

The batch set is intentionally one clean deliverable per target. If you want a different vertical layout (crop or triptych instead of blur), render that one individually from the **Render** panel's preset dropdown — or ask for the batch set to be adjusted.
