# Delivery Presets

A delivery preset re-encodes your moshed timeline into a clean H.264 master shaped for a specific platform — correct aspect ratio, resolution, bitrate, colour, and a sane keyframe interval. Pick one from the **Preset** dropdown in the Render panel.

## The presets

| Preset | Canvas | Layout |
|---|---|---|
| **Raw mosh** | source | Direct remux, no re-encode (the default) |
| **Instagram Reels — Blur BG** | 1080×1920 | Blurred enlarged background fills the bars; whole frame kept |
| **Instagram Reels — Crop** | 1080×1920 | Centre-cropped to fill |
| **Instagram Reels — Triptych** | 1080×1920 | Three stacked copies |
| **Instagram Feed — Square** | 1080×1080 | Centre-cropped square |
| **Instagram Feed — Landscape** | 1080×608 | Fit 16:9, no crop |
| **YouTube 1080p** | 1920×1080 | High-bitrate master |
| **YouTube 4K — anti-compression** | 3840×2160 | Lanczos upscale (see below) |

The layouts come straight from the kind of FFmpeg filter chains glitch artists already use for Instagram — **blur**, **crop**, **triptych**, **square** — so a 16:9 mosh maps onto a vertical or square canvas the way you'd expect.

## What every re-encode preset bakes in

These are the settings that keep platforms from blocking up your glitch:

- **Visually-lossless master quality** — CRF 16–17 (the YouTube presets add a generous `maxrate`). You hand the platform a near-pristine source.
- **Closed ~2-second GOP** — `-g 2×fps -keyint_min 2×fps -sc_threshold 0`. Predictable keyframes for the platform's segmenter.
- **`yuv420p`, High profile** — universal compatibility.
- **BT.709 colour tags** on the 1080+ targets — no colour shift on ingest.
- **`+faststart`** — the MP4 streams immediately.
- **AAC** audio at 256 kbps (384 for YouTube).
- **B-frames allowed.** The "no B-frames" rule only applies to the *moshable working clips*; the final master is never moshed again, so x264 is free to use B-frames for cleaner compression.

## Aspect & resolution

The project is **1920×1080** internally. Vertical and square presets fit/crop from that; the landscape and 1080p presets are essentially native. The **YouTube 4K** preset is the one that upscales — for a specific, deliberate reason covered in [Beating Platform Compression](platform-compression.md).

## Choosing a layout

- **Reels — Blur BG** is the safe default for vertical: nothing is cropped, and a blurred background reads as intentional.
- **Reels — Crop** when filling the frame matters more than keeping the edges.
- **Reels — Triptych** for a stacked, kaleidoscopic vertical.
- **Feed — Square / Landscape** for grid posts.
- **YouTube 1080p** for a straight high-quality upload; **YouTube 4K** when you want the best possible quality at the cost of a bigger file.
