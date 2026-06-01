# How Datamoshing Works

Datamoshing is a glitch technique that exploits how modern video compression *predicts* motion. To control it, it helps to understand what the codec is actually doing.

## I-frames and P-frames

H.264 (and most video codecs) don't store every frame as a full picture. That would be enormous. Instead they store two main kinds of frame:

- **I-frame (intra / keyframe)** — a complete, standalone picture. It decodes by itself, like a JPEG.
- **P-frame (predicted)** — *not* a picture. It stores **motion vectors** and small corrections that say: "take the previous frame and move these blocks around like *this*." A P-frame is meaningless without the frame before it.

A normal video is a steady rhythm of `I P P P P I P P P P …` — a keyframe every second or two, with P-frames filling the gaps.

## The trick

What happens if you **remove an I-frame** but keep the P-frames that followed it?

The decoder still has the P-frames' motion vectors — "move these blocks this way" — but the picture they were meant to move is gone. So it applies that motion to **whatever picture happens to be on screen**: the tail end of the *previous* shot.

The motion of the new shot drives the pixels of the old one. Colours smear along motion paths, objects melt and bloom, and the image slowly dissolves from one scene into another. That's the datamosh "bloom."

rustjay-mosh does exactly this, deliberately and reversibly: it **drops a clip's leading keyframe** at a cut so the incoming P-frames decode against the outgoing clip. See [Cross-Clip Mosh](../moshing/README.md).

## Why import re-transcodes everything

A random MP4 has keyframes scattered throughout, which makes precise moshing unpredictable. So on import rustjay-mosh transcodes every source to a **long-GOP** layout:

```
ffmpeg -i input -vf scale=1920:1080 -vcodec libx264 -g 99999999 -bf 0 …
```

- **`-g 99999999`** — a gigantic GOP ("group of pictures") size, so the encoder emits **one** keyframe at the very start and makes *everything else* a P-frame.
- **`-bf 0`** — no B-frames (which predict from *both* directions and complicate the bleed).

The payoff: every clip is now "one keyframe + a long tail of P-frames." Dropping that single keyframe turns the entire clip into pure motion that bleeds against whatever came before.

## What this means in practice

- The longer a clip's P-frame tail, the longer the bleed before the image stabilises.
- A clip's first frame is special — it's the only keyframe. Moshing targets it.
- Because the glitch is produced by the *decoder*, the final pixels are real picture data. That matters for export: re-encoding a moshed clip **preserves** the look (see [Beating Platform Compression](../exporting/platform-compression.md)).

Next: [The Packet Model](packet-model.md), the data structure all of this runs on.
