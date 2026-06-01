# Arranging Clips

The timeline runs along the bottom of the window: a **ruler** on top, a **video lane** (two tracks), and an **audio lane** below it. Everything is measured in **frames** at the project FPS.

## Placing clips

Drag a clip from the **Clip Pool** onto a lane. Video clips land on the video lane, audio clips on the audio lane. Where you drop sets the start frame and — for video — which of the two tracks it lands on.

## Moving

Drag a clip's **body** to slide it along the timeline. Remember the two tracks: **track 0 is on top** and takes render priority where clips overlap.

## The playhead

Click anywhere on the **ruler** (or on empty track space) to move the playhead. The centre preview decodes the frame under it. Drag along the ruler to scrub continuously.

## Selecting a range

Drag **on the ruler** to mark an inclusive range. The selection is the target for effects — when you bake a databend or compression effect, it applies to the selected range intersected with the selected clip. Use **Clear selection** in the Timeline panel to drop it.

## Zoom & pan

| Action | Gesture |
|---|---|
| Zoom | `Ctrl` + scroll, or the **− / +** buttons / slider |
| Pan | scroll |

Zoom ranges from a whole-project overview down to individual frames, so you can place a mosh exactly on the frame you want.

## Selecting clips

Click a clip's **body** to select it (the topmost clip wins when they're stacked). The selected clip is what the **Operations** panel acts on — mosh, bake, and so on. Press **Delete** to remove selected clips.

## Duplicating clips

There are two ways to duplicate a clip:

- **Alt-drag** (Option-drag on macOS) a clip's **body** to create a copy that follows your mouse from the original position. Release to drop it wherever you want. This works for both video and audio clips.
- **Ctrl+D** (Cmd+D on macOS) to duplicate the selected clip directly after itself. The new copy is automatically selected, so pressing **Ctrl+D again** chains another duplicate — handy for repeating a clip or building a stutter effect.

Duplicates inherit every property of the original: trim in/out, track assignment, colour, and mosh state. The copy gets a fresh ID and is validated for mosh adjacency as soon as you drop or place it.

Read on for [Trimming & Snapping](trimming.md) and [Audio Tracks & Fades](audio.md).
