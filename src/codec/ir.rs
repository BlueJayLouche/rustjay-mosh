/// YUV420 planar image.
#[derive(Debug, Clone)]
pub struct Yuv420 {
    pub width: u32,
    pub height: u32,
    /// Luma plane: width * height bytes.
    pub y: Vec<u8>,
    /// Chroma-blue plane: (width/2) * (height/2) bytes.
    pub u: Vec<u8>,
    /// Chroma-red plane: (width/2) * (height/2) bytes.
    pub v: Vec<u8>,
}

impl Yuv420 {
    pub fn new(width: u32, height: u32) -> Self {
        let luma = (width * height) as usize;
        let chroma = ((width / 2) * (height / 2)) as usize;
        Self {
            width,
            height,
            y: vec![0u8; luma],
            u: vec![128u8; chroma],
            v: vec![128u8; chroma],
        }
    }

    pub fn chroma_width(&self) -> u32 {
        self.width / 2
    }

    pub fn chroma_height(&self) -> u32 {
        self.height / 2
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MacroblockSize {
    Mb8x8 = 8,
    Mb16x16 = 16,
}

/// Per-macroblock motion vector.
#[derive(Debug, Clone, Copy, Default)]
pub struct MotionVector {
    pub dx: i16,
    pub dy: i16,
    /// Block column index (in macroblock units).
    pub bx: u16,
    /// Block row index (in macroblock units).
    pub by: u16,
}

/// Per-macroblock residual deltas (i16 per sample).
#[derive(Debug, Clone, Default)]
pub struct Residual {
    pub data: Vec<i16>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameType {
    /// Full intra-coded frame.
    I = 0,
    /// Predictive frame referencing a prior frame.
    P = 1,
}

impl TryFrom<u32> for FrameType {
    type Error = crate::format::binary::FormatError;

    fn try_from(v: u32) -> Result<Self, Self::Error> {
        match v {
            0 => Ok(FrameType::I),
            1 => Ok(FrameType::P),
            _ => Err(crate::format::binary::FormatError::UnknownFrameType(v)),
        }
    }
}

/// A decoded frame in the internal representation.
#[derive(Debug, Clone)]
pub struct Frame {
    pub frame_type: FrameType,
    /// Presentation timestamp (in ticks; interpretation is asset-specific).
    pub pts: u64,
    /// Macroblock size shared by all blocks in this frame's asset.
    pub mb_size: MacroblockSize,
    /// Index into the frame table of the reference frame (P-frames only).
    pub reference: Option<u32>,
    /// Full planes present for I-frames.
    pub planes: Option<Yuv420>,
    /// One entry per macroblock, ordered left-to-right, top-to-bottom.
    pub motion_vectors: Vec<MotionVector>,
    /// One residual per macroblock; layout: [Y: mb²][U: cmb²][V: cmb²] where cmb = mb/2.
    pub residuals: Vec<Residual>,
}
