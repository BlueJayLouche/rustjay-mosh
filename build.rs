fn main() {
    #[cfg(target_os = "windows")]
    windows::copy_ffmpeg_dlls();
}

#[cfg(target_os = "windows")]
mod windows {
    use std::path::{Path, PathBuf};

    /// Copy FFmpeg (and transitive) DLLs into the cargo output directory so the
    /// binary can find them at runtime without requiring them in PATH.
    ///
    /// Search order:
    ///   1. `FFMPEG_DIR` env var   – set by the release CI (BtbN FFmpeg package)
    ///   2. `MSYS2_ROOT` env var   – defaults to `C:\msys64`
    pub fn copy_ffmpeg_dlls() {
        println!("cargo:rerun-if-env-changed=FFMPEG_DIR");
        println!("cargo:rerun-if-env-changed=MSYS2_ROOT");

        let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR not set");
        // OUT_DIR is  target/{profile}/build/{crate}-{hash}/out
        // The binary lands in  target/{profile}
        let profile_dir = PathBuf::from(&out_dir)
            .ancestors()
            .nth(3)
            .expect("unexpected OUT_DIR depth")
            .to_path_buf();

        let (src, all_dlls) = match find_ffmpeg_bin() {
            Some(pair) => pair,
            None => {
                println!(
                    "cargo:warning=FFmpeg DLLs not found. \
                     Set FFMPEG_DIR to a shared FFmpeg 7.x build or install MSYS2 \
                     with the mingw-w64-x86_64-ffmpeg package. \
                     The app will fail to start without them."
                );
                return;
            }
        };

        let n = copy_dlls(&src, &profile_dir, all_dlls);
        if n > 0 {
            println!("cargo:warning=Copied {n} FFmpeg DLLs from {} to {}", src.display(), profile_dir.display());
        }
    }

    /// Returns `(dll_dir, copy_all)`.
    /// `copy_all = true`  → copy every .dll in the directory (clean FFmpeg package).
    /// `copy_all = false` → copy only DLLs whose names match the FFmpeg/codec pattern.
    fn find_ffmpeg_bin() -> Option<(PathBuf, bool)> {
        // 1. Explicit FFMPEG_DIR (CI and informed users)
        if let Ok(dir) = std::env::var("FFMPEG_DIR") {
            let bin = PathBuf::from(dir).join("bin");
            if bin.join("avcodec-61.dll").exists() {
                return Some((bin, true)); // clean package — copy everything
            }
        }

        // 2. MSYS2 (common Windows dev setup)
        let msys_root = std::env::var("MSYS2_ROOT")
            .unwrap_or_else(|_| "C:\\msys64".to_string());
        let bin = PathBuf::from(msys_root).join("mingw64").join("bin");
        if bin.join("avcodec-61.dll").exists() {
            return Some((bin, false)); // large shared dir — only copy codec DLLs
        }

        None
    }

    /// Copy DLLs from `src` into `dst`.
    /// When `all` is false, only DLLs whose names start with a codec-related prefix
    /// are copied (avoids pulling in unrelated MSYS2 tools like Python, GCC, etc.).
    fn copy_dlls(src: &Path, dst: &Path, all: bool) -> usize {
        let Ok(entries) = std::fs::read_dir(src) else {
            return 0;
        };

        let mut count = 0;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("dll") {
                continue;
            }
            let name = entry.file_name();
            let name_lower = name.to_string_lossy().to_lowercase();

            if !all && !is_ffmpeg_related(&name_lower) {
                continue;
            }

            let dest = dst.join(&name);
            // Don't overwrite if identical size to avoid unnecessary rebuilds.
            if let (Ok(src_meta), Ok(dst_meta)) =
                (std::fs::metadata(&path), std::fs::metadata(&dest))
            {
                if src_meta.len() == dst_meta.len() {
                    continue;
                }
            }

            if std::fs::copy(&path, &dest).is_ok() {
                count += 1;
            }
        }
        count
    }

    /// Returns true for DLL names that belong to the FFmpeg/codec ecosystem.
    /// This covers all transitive runtime dependencies discovered by walking the
    /// PE import table of avcodec, avformat and their codec plugins.
    fn is_ffmpeg_related(name: &str) -> bool {
        // FFmpeg libraries themselves
        if name.starts_with("av")
            || name.starts_with("sw")
            || name.starts_with("postproc")
        {
            return true;
        }
        // Codec and support libraries (lib* covers ~95 % of transitive deps)
        if name.starts_with("lib") {
            return true;
        }
        // A handful that don't follow the lib* convention
        matches!(
            name,
            "zlib1.dll" | "sdl2.dll" | "xvidcore.dll" | "z.dll"
        )
    }
}
