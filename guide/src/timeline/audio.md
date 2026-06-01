# Audio Tracks & Fades

rustjay-mosh isn't just video — it has an audio lane so your mosh can land on a beat and your cuts can have sound.

## Importing audio

Import an audio file (`wav`, `mp3`, `aac`, `flac`, `m4a`, `ogg`) the same way as video, via **➕ Import clip**. You can also import a video that *contains* audio — the audio track is extracted alongside the video clip and placed on the audio lane at the same position.

Internally audio is decoded to **48 kHz stereo float** samples, and a per-video-frame **peak** envelope is computed so the waveform you see on the lane lines up frame-for-frame with the video above it.

## Placing & trimming

Drag an audio clip from the pool onto the **audio lane**. Drag its body to move it; drag an **edge** to trim — exactly like video clips.

## Fades and crossfades

Audio clips support fades, controlled by a modifier gesture:

| Action | Gesture |
|---|---|
| Trim | drag an edge (no modifier) |
| Fade **in** | **Shift** + drag rightward on the clip's **left half** |
| Fade **out** | **Shift** + drag leftward on the clip's **right half** |

When two audio clips are butted together and their fade regions overlap, the fades become an automatic **crossfade** — the outgoing clip ramps down as the incoming clip ramps up across the overlap. No extra step needed; just give adjacent clips a fade-out and fade-in.

## How audio reaches the render

At render time the audio lane is mixed down to a single 48 kHz stereo WAV — applying every fade and crossfade — and muxed into the output MP4 as AAC. If there's no audio on the timeline, the render is video-only. See [Rendering](../exporting/README.md).
