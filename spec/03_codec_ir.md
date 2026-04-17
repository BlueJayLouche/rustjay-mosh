# Codec / IR Specification

## Pixel Format

-   YUV420 planar

## Macroblocks

-   8x8 or 16x16 (fixed per asset)

## Frame Types

-   I: full planes
-   P: motion + residual

## Residuals

-   Raw i16 deltas

## Motion Vectors

-   Per block (dx, dy)
