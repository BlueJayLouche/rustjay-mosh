# Trimming & Snapping

## Trimming

Drag a clip's **left edge** to trim the in-point, or the **right edge** to trim the out-point.

In packet terms (see [The Packet Model](../core-concepts/packet-model.md)):

- Trimming **in** increases `source_offset` — the number of head packets skipped — and shrinks the visible count.
- Trimming **out** just shrinks the visible count.

Nothing is destroyed; trimming only changes which slice of the source packets plays. Drag the edge back out to recover the frames.

## Snapping

Edges **snap to other clip boundaries** as you drag, so you can butt two clips together exactly with no gap or overlap. This matters a lot for moshing: a cross-clip mosh only bleeds if the moshed clip starts on the **exact frame** the previous clip ends. Snapping makes that one-frame alignment trivial.

## Trimming and the mosh keyframe

There's a subtle interaction worth knowing. A clip's only keyframe is its **first** packet. If you trim a moshed clip's in-point, you're skipping packets from the head — including, potentially, the keyframe the mosh was meant to drop.

rustjay-mosh keeps this consistent for you. The packet iterator accounts for the dropped keyframe when computing how many frames are visible, and the app re-validates mosh state whenever you move or trim. If a moshed clip ends up no longer butted against a predecessor, the mosh is automatically disabled and the clip's length restored — so you never get a silently broken render. See [Dropping Keyframes](../moshing/keyframes.md) for the details.
