use std::io::{self, Read, Write};

use thiserror::Error;

pub const MAGIC: &[u8; 4] = b"MOSH";
pub const VERSION_MAJOR: u16 = 0;
pub const VERSION_MINOR: u16 = 2;

#[derive(Debug, Error)]
pub enum FormatError {
    #[error("invalid magic bytes")]
    BadMagic,
    #[error("unsupported version {0}.{1}")]
    UnsupportedVersion(u16, u16),
    #[error("unknown frame type {0}")]
    UnknownFrameType(u32),
    #[error("i/o error: {0}")]
    Io(#[from] io::Error),
}

/// Top-level file header.
#[derive(Debug, Clone)]
pub struct FileHeader {
    pub version_major: u16,
    pub version_minor: u16,
    pub frame_count: u64,
}

impl FileHeader {
    /// 16 bytes: 4 magic + 2 major + 2 minor + 8 frame_count.
    pub fn write_to<W: Write>(&self, w: &mut W) -> Result<(), FormatError> {
        w.write_all(MAGIC)?;
        w.write_all(&self.version_major.to_le_bytes())?;
        w.write_all(&self.version_minor.to_le_bytes())?;
        w.write_all(&self.frame_count.to_le_bytes())?;
        Ok(())
    }

    pub fn read_from<R: Read>(r: &mut R) -> Result<Self, FormatError> {
        let mut magic = [0u8; 4];
        r.read_exact(&mut magic)?;
        if &magic != MAGIC {
            return Err(FormatError::BadMagic);
        }
        let mut buf2 = [0u8; 2];
        r.read_exact(&mut buf2)?;
        let version_major = u16::from_le_bytes(buf2);
        r.read_exact(&mut buf2)?;
        let version_minor = u16::from_le_bytes(buf2);
        if version_major != VERSION_MAJOR {
            return Err(FormatError::UnsupportedVersion(version_major, version_minor));
        }
        let mut buf8 = [0u8; 8];
        r.read_exact(&mut buf8)?;
        let frame_count = u64::from_le_bytes(buf8);
        Ok(Self { version_major, version_minor, frame_count })
    }
}

/// 32-byte frame table entry.
#[derive(Debug, Clone, Copy)]
pub struct FrameTableEntry {
    pub offset: u64,
    pub size: u64,
    pub pts: u64,
    pub frame_type: u32,
    pub reference: u32,
}

impl FrameTableEntry {
    pub fn write_to<W: Write>(&self, w: &mut W) -> Result<(), FormatError> {
        w.write_all(&self.offset.to_le_bytes())?;
        w.write_all(&self.size.to_le_bytes())?;
        w.write_all(&self.pts.to_le_bytes())?;
        w.write_all(&self.frame_type.to_le_bytes())?;
        w.write_all(&self.reference.to_le_bytes())?;
        Ok(())
    }

    pub fn read_from<R: Read>(r: &mut R) -> Result<Self, FormatError> {
        let mut buf8 = [0u8; 8];
        let mut buf4 = [0u8; 4];

        r.read_exact(&mut buf8)?;
        let offset = u64::from_le_bytes(buf8);
        r.read_exact(&mut buf8)?;
        let size = u64::from_le_bytes(buf8);
        r.read_exact(&mut buf8)?;
        let pts = u64::from_le_bytes(buf8);
        r.read_exact(&mut buf4)?;
        let frame_type = u32::from_le_bytes(buf4);
        r.read_exact(&mut buf4)?;
        let reference = u32::from_le_bytes(buf4);

        Ok(Self { offset, size, pts, frame_type, reference })
    }
}

/// A single motion vector as serialised in a frame block.
/// Layout: i16 dx, i16 dy, u16 bx, u16 by (8 bytes total).
#[derive(Debug, Clone, Copy)]
pub struct RawMotionVector {
    pub dx: i16,
    pub dy: i16,
    pub bx: u16,
    pub by: u16,
}

impl RawMotionVector {
    pub fn write_to<W: Write>(&self, w: &mut W) -> Result<(), FormatError> {
        w.write_all(&self.dx.to_le_bytes())?;
        w.write_all(&self.dy.to_le_bytes())?;
        w.write_all(&self.bx.to_le_bytes())?;
        w.write_all(&self.by.to_le_bytes())?;
        Ok(())
    }

    pub fn read_from<R: Read>(r: &mut R) -> Result<Self, FormatError> {
        let mut buf2 = [0u8; 2];
        r.read_exact(&mut buf2)?;
        let dx = i16::from_le_bytes(buf2);
        r.read_exact(&mut buf2)?;
        let dy = i16::from_le_bytes(buf2);
        r.read_exact(&mut buf2)?;
        let bx = u16::from_le_bytes(buf2);
        r.read_exact(&mut buf2)?;
        let by = u16::from_le_bytes(buf2);
        Ok(Self { dx, dy, bx, by })
    }
}
