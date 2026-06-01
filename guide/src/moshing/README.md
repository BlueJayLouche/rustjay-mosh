# Cross-Clip Mosh

This is the headline move: make one clip **bleed into** the clip before it.

## What it does

Select a clip that sits **immediately after** another clip on the timeline, then click **Cross-Clip Mosh** in the Operations panel. The app sets the clip's `drop_leading_keyframe` flag and trims one frame off its count to keep timing aligned.

On playback and render, the decoder reaches the cut, finds the moshed clip's keyframe **missing**, and applies its incoming P-frame motion to the last decoded picture of the previous clip. The previous shot smears along the new shot's motion — the datamosh bloom. (The mechanism is explained in [How Datamoshing Works](../core-concepts/README.md).)

## The one rule: clips must touch

A cross-clip mosh only works if the moshed clip starts on the **exact frame** the previous clip ends — they must be adjacent with no gap. If there's a gap, there's no "previous picture" to bleed from at that moment.

Use **edge snapping** (see [Trimming & Snapping](../timeline/trimming.md)) to butt them together precisely. The Operations panel will tell you when there's no valid predecessor to mosh against.

## Composing moshes

A few things to try:

- **Chain them.** Mosh several consecutive cuts so each shot dissolves into the next in one long bloom.
- **Mosh a baked clip.** Bake a databend or compression effect (see [Glitch Effects](../glitch/README.md)) and mosh *that* — the motion of corrupted data is its own aesthetic.
- **Vary the tail length.** A longer clip after the cut means a longer bleed before the picture restabilises; trim the moshed clip shorter for a quick stutter, longer for a slow melt.
- **Layer tracks.** Put a clean take on track 1 and a moshed take on track 0 so the glitch sits over the original.

## It's non-destructive

Moshing only flips a flag on a timeline placement. Click it again to un-mosh, or undo. The underlying pool clip is never modified, so you can mosh, compare, and back out freely.

Next: [Dropping Keyframes](keyframes.md) — what the app does to keep moshes valid as you edit.
