# The Packet Model

rustjay-mosh never works on decoded pixels for editing — it works on **encoded packets**. This is what makes lossless, re-encode-free moshing possible.

## OwnedPacket

When a clip is imported, the transcoded MP4 is read back not as frames but as a list of raw H.264 **packets**:

```rust
pub struct OwnedPacket {
    pub data: Vec<u8>,   // the raw encoded bytes of one frame
    pub pts: i64,        // presentation timestamp
    pub dts: i64,        // decode timestamp
    pub duration: i64,
    pub is_key: bool,    // true only for the leading I-frame
}
```

A packet is the codec's compressed representation of a single frame. The editor moves, copies, drops, and reorders these byte blobs **without decoding them**. Only the preview decodes — and only for display.

## PacketClip

A whole imported source is a `PacketClip`: its name, dimensions, the codec parameters and timebase needed to decode or remux it, and the packet list:

```rust
pub struct PacketClip {
    pub id: u64,
    pub name: String,
    pub packets: Vec<OwnedPacket>,   // [keyframe, P, P, P, …]
    pub width: u32,
    pub height: u32,
    pub codec_parameters: ffmpeg::codec::Parameters,
    pub time_base: ffmpeg::Rational,
}
```

Because every clip was transcoded with `-g 99999999 -bf 0`, exactly one packet in the list has `is_key == true`: the first one.

## Spans: how a timeline clip maps to packets

A clip on the timeline is a **view** into a `PacketClip`, not a copy. A `ClipSpan` describes which slice of packets is visible and whether to mosh:

- **`source_offset`** — how many packets to skip from the head (this is your trim-in).
- **`visible_count`** — how many packets to play.
- **`drop_leading_keyframe`** — if `true`, skip the first visible keyframe and start at the first P-frame. **This flag is the mosh.**

When you render, all the spans on the timeline are flattened into one flat `Vec<OwnedPacket>` in playback order.

## Rendering = remuxing

The default render path doesn't re-encode. It takes that flat packet list, **rewrites the timestamps** to be monotonic across the whole sequence (so the file is well-formed even though packets came from many clips), and writes them into an MP4 container with `av_interleaved_write_frame`. The encoded bytes are copied through untouched.

That's why a Raw-mosh export is pixel-exact with the preview: nothing was decoded and re-encoded in between. (The [delivery presets](../exporting/delivery-presets.md) are the deliberate exception — they *do* re-encode, on purpose, for the platforms.)

Next: [Clips & the Pool](clips-and-pool.md).
