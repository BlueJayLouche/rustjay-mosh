# Beating Platform Compression

The single most common complaint about datamosh footage online: *"it looked great on my machine, then Instagram/YouTube turned it into blocky mush."* This page explains why that happens and how rustjay-mosh's delivery presets fix it.

## Why a Raw mosh blocks up after upload

A Raw-mosh export is a [direct packet remux](README.md): one keyframe followed by thousands of P-frames, carrying deliberately-broken frame references. When you upload it, the platform's encoder has to re-compress it — and it's working from:

- **no clean reference frames** (you removed them — that's the mosh), and
- **already-degraded content** (the bleed and smear).

So its rate-control panics and spends bits badly, and you get compounding macroblocks on top of your glitch. The platform isn't being malicious; you handed it the worst possible input.

## The key insight: the glitch is in the pixels

Here's what makes the fix possible. The datamosh bloom is produced by the **decoder** applying wrong motion to a real picture. By the time a frame is decoded, the smear is **actual pixel data** — not some fragile property of the bitstream.

That means you can **decode and re-encode a moshed clip and the glitch is preserved.** It becomes ordinary picture content that any encoder will reproduce faithfully.

## The fix: encode the master yourself

So instead of letting the platform re-encode a fragile stream, the delivery presets re-encode it **first**, at high quality, with platform-friendly structure:

- A near-lossless CRF master → the platform compresses from a pristine source, not a broken one.
- A closed 2-second GOP with real keyframes → the platform's segmenter has clean cut points.
- Correct colour tags, `yuv420p`, faststart → nothing surprises the ingester.

You're not avoiding the platform's re-encode — that's unavoidable. You're making sure it starts from the best possible input, so its output stays clean.

## The YouTube trick: upload bigger than you need

YouTube allocates bitrate by **resolution tier**, not by how much detail the content actually has. A 1080p upload gets the stingy AVC budget — the classic blocky YouTube look. But cross into **1440p or 4K** and YouTube switches the video to the **VP9/AV1** codec with a far larger bitrate ceiling — and it serves that better encode at *every* playback resolution, including 1080p.

So the **YouTube 4K — anti-compression** preset upscales your 1080p master to 2160p with a sharp lanczos scaler before export. The file is bigger, but YouTube encodes it in its high-quality tier and your glitch looks dramatically cleaner even to viewers watching at 1080p.

The YouTube presets also:

- target generous bitrates near YouTube's *recommended upload* specs, so the master isn't starved, and
- use `aq-mode=3` so x264 spends bits on the flat gradients and smears where blocking shows worst.

## Rule of thumb

> **Preview and archive in Raw mosh. Post through a delivery preset.** For YouTube specifically, post through **4K — anti-compression** unless file size is a hard constraint.

Next: [Export for All Platforms](export-all.md) to produce every master in one click.
