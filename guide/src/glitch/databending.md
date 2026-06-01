# Databending

Databending treats the decoded image as raw bytes and mangles them directly. rustjay-mosh applies these operations to the `YUV420` planes of each frame in the selected range, then [bakes](README.md) the result back to H.264.

Open the **Databend** dialog from the Operations panel, pick a mode, set its parameters, and bake.

## Modes

### Reverse scanlines

Reverses the byte order of every row in the luma (Y) plane. Each scanline is mirrored horizontally, giving a torn, flipped-strip look while colour stays roughly in place.

### Echo

Repeats earlier rows over later ones at a chosen **delay**, mixed at a **mix** ratio. Produces a vertical smear / ghosting down the frame — like a feedback trail in the spatial domain.

- **delay** — how many rows back the echo is pulled from.
- **mix** — `0.0` = no echo, `1.0` = full replacement.

### Bitcrush

Masks off the low bits of **every** plane, quantising the image to `bits` significant bits. Lower values posterise harder — banding, flattened colour, chunky gradients. Applies to Y, U and V alike, so colour quantises too.

### Byte-swap

Reverses each `stride`-byte chunk of every plane. Small strides shuffle fine detail; large strides displace whole blocks. A blocky, scrambled texture that reads as "corrupted data."

### XOR

XORs every byte of every plane with a **mask**. Inverts and remaps values — high masks can flip the image into vivid false colour and inverted luma. Cheap, deterministic, and aggressive.

### Noise

Adds deterministic pseudo-random noise in the range ±`amount`, seeded by a **seed** value. Because it's seeded, the same seed always produces the same noise — so a baked result is reproducible. Use it for grain, static, and signal-degradation looks.

## Tips

- **Bend, then mosh.** A databent clip moshed against a clean one bleeds *corrupted* motion — two glitch techniques compounding.
- **Keep bakes short for hits.** A one-frame bitcrush or XOR in the middle of a clean run reads as a sharp data "spike."
- **Stack modes** by baking one effect, then baking another over the result.
