pub mod audio;
pub mod bake;
pub mod codec;
pub mod datamosh;
pub mod format;
pub mod frame_graph;
pub mod importer;
pub mod packet;
pub mod preview;
pub mod render;
pub mod timeline;
pub mod ui;

pub use codec::ir::Yuv420;
pub use format::binary::{FileHeader, FrameTableEntry};
pub use frame_graph::FrameGraph;

/// Returns the bundled `ffmpeg` binary if it lives next to the executable
/// (macOS .app, Windows zip), otherwise falls back to `ffmpeg` on PATH.
pub fn bundled_ffmpeg() -> std::ffi::OsString {
    if let Ok(mut path) = std::env::current_exe() {
        path.pop();
        let candidate = path.join(if cfg!(windows) { "ffmpeg.exe" } else { "ffmpeg" });
        if candidate.exists() {
            return candidate.into_os_string();
        }
    }
    std::ffi::OsString::from("ffmpeg")
}
