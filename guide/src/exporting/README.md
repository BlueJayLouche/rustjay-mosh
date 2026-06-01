# Rendering

Rendering turns your timeline into a finished MP4. rustjay-mosh has **two** render paths, and choosing the right one is the difference between a glitch that survives upload and one that turns to mush.

Open the **Render** panel on the right, set the output **FPS**, choose an **export preset**, and click **🎬 Render to file…**.

## The two paths

### Raw mosh — direct remux (default)

The **Raw mosh** preset doesn't re-encode. The timeline's packets are flattened into one sequence, timestamps are rewritten to be monotonic, and the bytes are remuxed straight into an MP4 (`-c:v copy`). See [The Packet Model](../core-concepts/packet-model.md).

- **Pixel-exact** with what you previewed.
- **Fast** — no encoding.
- **Fragile for upload.** It's a one-keyframe, thousands-of-P-frames stream. Platforms re-compress it from no clean reference and the result blocks up badly.

Use Raw mosh for archival, for handing the file to another editor, or when *you* control the final encode.

### Delivery presets — re-encode for a platform

The Instagram and YouTube presets **re-encode** the output into a clean, platform-shaped H.264 master. Because the [mosh glitch lives in the pixels](platform-compression.md), it survives the re-encode — but now you hand the platform a pristine master instead of a fragile bitstream.

This is the path to use for anything you're posting. Details in [Delivery Presets](delivery-presets.md).

## Audio

If the timeline has audio, the lane is mixed to a 48 kHz stereo WAV (applying fades and crossfades) and muxed into the output as **AAC** — 256 kbps, or 384 kbps for the YouTube presets. No audio on the timeline means a video-only file.

## How the render runs

Rendering happens on a background thread so the UI stays responsive; the status line reports progress. Under the hood:

1. The flat packet sequence is exported to a temp `video.mp4` (remux, no re-encode).
2. If there's audio, it's mixed to `audio.wav`.
3. A final FFmpeg pass produces the output — `-c:v copy` for Raw mosh, or the preset's encode arguments otherwise — muxing in the audio and writing `+faststart`.

> **Re-encode presets take longer than Raw mosh.** They run a full x264 encode over the whole sequence (the status reads *"Encoding delivery master…"*). That's expected.

Read [Delivery Presets](delivery-presets.md) next, and [Beating Platform Compression](platform-compression.md) for *why* it works.
