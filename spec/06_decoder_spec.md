# Decoder Spec

## Algorithm

for each frame: if I: return planes else: ref = decode(reference) for
block: sample(ref, dx, dy) add residual clamp 0-255

## Chroma

dx/2, dy/2

## Sampling

Clamp to edges
