use std::collections::HashMap;

use crate::codec::ir::{Frame, MacroblockSize};

/// A media asset: a decoded sequence of frames stored in the internal IR.
#[derive(Debug)]
pub struct Asset {
    pub id: u32,
    pub name: String,
    pub mb_size: MacroblockSize,
    pub frames: Vec<Frame>,
}

/// Central store for all imported media.
#[derive(Debug, Default)]
pub struct MediaPool {
    assets: HashMap<u32, Asset>,
    next_id: u32,
}

impl MediaPool {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a new asset and return its assigned id.
    pub fn add_asset(&mut self, name: impl Into<String>, mb_size: MacroblockSize, frames: Vec<Frame>) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        self.assets.insert(id, Asset { id, name: name.into(), mb_size, frames });
        id
    }

    pub fn get(&self, id: u32) -> Option<&Asset> {
        self.assets.get(&id)
    }

    pub fn get_frame(&self, asset_id: u32, frame_idx: u32) -> Option<&Frame> {
        self.assets.get(&asset_id)?.frames.get(frame_idx as usize)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Asset> {
        self.assets.values()
    }
}
