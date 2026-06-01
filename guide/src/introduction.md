<div class="rj-hero">
  <span class="rj-corner-bl"></span><span class="rj-corner-br"></span>
  <div class="rj-hero-meta">
    <span>NODE // RUSTJAY · MOSH</span>
    <span class="rj-status">SYSTEM ONLINE</span>
  </div>
  <div class="rj-hero-inner">
    <div class="rj-logo">RUSTJAY<span class="rj-logo-accent">/</span>MOSH
      <span class="rj-logo-sub">DATAMOSH NON-LINEAR EDITOR</span>
    </div>
    <p class="rj-tagline">A Rust desktop editor for <strong>datamoshing</strong> — glitch-art video built by breaking the codec on purpose. Import, arrange, mosh, bend, and export clean platform masters that survive Instagram and YouTube.</p>
    <div class="rj-cta">
      <a href="installation.html" class="rj-btn rj-btn-primary">▸ Get Started</a>
      <a href="https://github.com/BlueJayLouche/rustjay-mosh" class="rj-btn rj-btn-ghost">GitHub ↗</a>
    </div>
  </div>
</div>

<div class="rj-features">
  <div class="rj-feature">
    <div class="rj-feature-icon">▣</div>
    <h3>I-Frame Moshing</h3>
    <p>Drop a clip's leading keyframe so its motion vectors decode against the wrong reference — the classic bleed-and-smear datamosh, on a timeline.</p>
  </div>
  <div class="rj-feature">
    <div class="rj-feature-icon">◉</div>
    <h3>Packet Remux</h3>
    <p>Renders by remuxing the manipulated H.264 packet stream directly — no re-encode, so the glitch you see is the glitch you keep.</p>
  </div>
  <div class="rj-feature">
    <div class="rj-feature-icon">〜</div>
    <h3>Databending & Bake</h3>
    <p>Scanline reversal, bitcrush, byte-swap, XOR, noise, and real JPEG macroblock artifacting — baked back into H.264 to mosh against.</p>
  </div>
  <div class="rj-feature">
    <div class="rj-feature-icon">⬡</div>
    <h3>GPU Preview</h3>
    <p>wgpu YUV→RGB preview on Metal, Vulkan, and DX12. Scrub the timeline and watch the decode bleed in real time.</p>
  </div>
  <div class="rj-feature">
    <div class="rj-feature-icon">◈</div>
    <h3>Delivery Presets</h3>
    <p>One-click Instagram (Reels / Feed) and YouTube masters — including a 4K upscale trick that escapes YouTube's blocky 1080p tier.</p>
  </div>
  <div class="rj-feature">
    <div class="rj-feature-icon">♩</div>
    <h3>Projects</h3>
    <p>Self-contained <code>.rjmosh</code> bundles, collect-to-zip sharing, autosave crash recovery, and full undo/redo.</p>
  </div>
</div>

---

## What is rustjay-mosh?

**rustjay-mosh** is a non-linear editor built for one thing: **datamoshing**. Where a normal editor works hard to keep video clean, rustjay-mosh gives you precise control over breaking it — dropping keyframes, bending bytes, and forcing compression artifacts — then arranges those broken clips on a timeline like any other NLE.

The workflow is short:

1. **Import** any video FFmpeg can read. It's transcoded to a long-GOP H.264 stream (one keyframe, all P-frames) so it's ready to mosh.
2. **Arrange** clips on a two-track timeline, trim them, snap them edge-to-edge.
3. **Mosh** — drop a clip's leading keyframe so it bleeds into the clip before it.
4. **Bend & bake** — apply databending or compression artifacts to a selection and re-encode it back into the timeline.
5. **Render** — remux straight to MP4, or re-encode through a **delivery preset** shaped for the platform you're posting to.

## Running it

```sh
git clone https://github.com/BlueJayLouche/rustjay-mosh
cd rustjay-mosh
cargo run --release
```

> **Release mode matters.** The codec paths are numeric-heavy; debug builds are noticeably slower for decode, bake, and render.

## How to use this guide

Start with [Installation](installation.md), then make something glitchy in [Your First Mosh](getting-started/README.md). After that the chapters are mostly independent — [Core Concepts](core-concepts/README.md) explains *why* the moshing works, and the [Exporting](exporting/README.md) section is the one to read before you post anywhere.
