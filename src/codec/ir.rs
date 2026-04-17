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
