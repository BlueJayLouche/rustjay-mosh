# Dropping Keyframes

A cross-clip mosh is, mechanically, a single decision: **skip the moshed clip's first visible keyframe** when building the packet sequence. This page covers what that means and how the app keeps it from breaking under editing.

## At the packet level

When `drop_leading_keyframe` is `true`, the clip's span starts at the **first P-frame** instead of the keyframe:

```text
clip packets:   [ I ][ P ][ P ][ P ] …
                  ↑ dropped
played:               [ P ][ P ][ P ] …
```

Those P-frames carry motion vectors but no self-contained picture, so the decoder applies them to whatever it last decoded — the previous clip's final frame.

The visible frame count is reduced by one to match the dropped packet, so the clip occupies the same span on the timeline and downstream timing stays correct.

## Why it has to stay adjacent

The bleed only exists because the previous clip left a picture in the decoder. If the moshed clip is no longer butted against a predecessor — say you dragged it away, or deleted the clip before it — there's nothing to bleed from, and a clip that begins on raw P-frames with no reference is just corrupt garbage at the wrong length.

## Automatic re-validation

rustjay-mosh guards against this. After **every drag release** and after **deletions**, it re-checks every moshed clip:

- Is this clip still immediately preceded by another clip (does some clip's end frame equal this clip's start frame)?
- If **yes**, the mosh stands.
- If **no**, the mosh is disabled, `drop_leading_keyframe` is cleared, and the one frame that was trimmed for the mosh is **restored**.

So you can rearrange freely. Moshes that still make sense are kept; moshes that have been orphaned are quietly undone with their timing repaired — you'll never render a clip whose keyframe was dropped with nothing to decode against.

> If you move a clip back into place, just click **Cross-Clip Mosh** again to re-apply it.
