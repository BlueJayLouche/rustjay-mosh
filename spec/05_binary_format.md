# Binary Format (v0.2)

## Endianness

Little-endian

## File Layout

\[Header\]\[Asset\]\[Frame Table\]\[Frames\]

## Header

Magic: MOSH Version: u16/u16 Frame count: u64

## Frame Table Entry (32 bytes)

-   offset u64
-   size u64
-   pts u64
-   type u32
-   ref u32

## Frame Block

\[Frame Header\]\[MV\]\[Residual\]\[Planes\]

## Motion Vector

i16 dx, i16 dy, u16 bx, u16 by

## Residual

Per block: \[u32 len\]\[i16 data...\]

## I-frame planes

Y + U + V
