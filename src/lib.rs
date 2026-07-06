pub mod audio;
pub mod bake;
pub mod codec;
pub mod importer;
pub mod packet;
pub mod preview;
pub mod project;
pub mod render;
pub mod ui;

pub use codec::ir::Yuv420;

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
