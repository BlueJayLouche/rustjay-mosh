//! Platform delivery-encode presets.
//!
//! The render path ([`crate::ui::app::MoshApp::start_render`]) finishes the
//! moshed sequence as a direct packet remux (`-c:v copy`): one keyframe + all
//! P-frames, with the deliberately-broken GOP that makes datamoshing work. That
//! bitstream is fragile — when a platform re-encodes it (no clean reference
//! frames, already-degraded content) the result is heavy blockiness.
//!
//! The fix is a *delivery-encode pass*. The mosh glitch lives in the decoded
//! **pixels**, not the bitstream structure, so re-encoding to a clean,
//! high-quality, platform-shaped H.264 master preserves the glitch look while
//! handing the platform something it can compress gracefully. We encode at the
//! quality the user's own ffmpeg scripts use (CRF ~16–17, `+faststart`,
//! `yuv420p`) plus a closed ~2 s GOP and per-platform canvas/layout.
//!
//! Every variant here translates one of the layouts from the user's
//! `glitchFuck_insta*.sh` scripts (`square`, `crop`, `blur`, `triptych`) into a
//! self-contained ffmpeg filter, and produces the full argument vector for the
//! final ffmpeg invocation via [`ExportPreset::build_ffmpeg_args`].

/// A user-selectable delivery target. The default, [`ExportPreset::RawMosh`],
/// reproduces the historical behaviour (direct remux, no re-encode).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ExportPreset {
    /// Direct remux of the moshed packets (`-c:v copy`). Pixel-exact mosh, but
    /// platforms will re-compress it and may introduce blockiness.
    RawMosh,
    /// Instagram Reels 9:16, blurred enlarged background fills the bars so the
    /// whole 16:9 frame is preserved (the script's `blur` layout).
    ReelsBlur,
    /// Instagram Reels 9:16, centre-cropped to fill (the script's `crop`).
    ReelsCrop,
    /// Instagram Reels 9:16, three stacked copies (the script's `triptych`).
    ReelsTriptych,
    /// Instagram Feed 1:1, centre-cropped square (the script's `square`).
    FeedSquare,
    /// Instagram Feed 16:9 landscape, fit with no crop.
    FeedLandscape,
    /// YouTube native 1080p master, generous bitrate.
    YouTube1080,
    /// YouTube 4 K (2160p) lanczos upscale — pushes the upload into YouTube's
    /// VP9/AV1 / high-bitrate tier so the 1080p playback is far less blocky.
    YouTube4K,
}

impl ExportPreset {
    /// All presets in menu order.
    pub const ALL: [ExportPreset; 8] = [
        ExportPreset::RawMosh,
        ExportPreset::ReelsBlur,
        ExportPreset::ReelsCrop,
        ExportPreset::ReelsTriptych,
        ExportPreset::FeedSquare,
        ExportPreset::FeedLandscape,
        ExportPreset::YouTube1080,
        ExportPreset::YouTube4K,
    ];

    /// Short label for the combo box.
    pub fn label(self) -> &'static str {
        match self {
            ExportPreset::RawMosh => "Raw mosh (no re-encode)",
            ExportPreset::ReelsBlur => "Instagram Reels — Blur BG (9:16)",
            ExportPreset::ReelsCrop => "Instagram Reels — Crop (9:16)",
            ExportPreset::ReelsTriptych => "Instagram Reels — Triptych (9:16)",
            ExportPreset::FeedSquare => "Instagram Feed — Square (1:1)",
            ExportPreset::FeedLandscape => "Instagram Feed — Landscape (16:9)",
            ExportPreset::YouTube1080 => "YouTube 1080p (16:9)",
            ExportPreset::YouTube4K => "YouTube 4K — anti-compression (16:9)",
        }
    }

    /// One-line caption shown under the selector.
    pub fn description(self) -> &'static str {
        match self {
            ExportPreset::RawMosh => {
                "Pixel-exact mosh. Platforms re-compress it — expect blockiness."
            }
            ExportPreset::ReelsBlur => {
                "1080×1920. Blurred background fills the bars; whole frame kept."
            }
            ExportPreset::ReelsCrop => "1080×1920. Centre-cropped to fill, edges lost.",
            ExportPreset::ReelsTriptych => "1080×1920. Three stacked copies.",
            ExportPreset::FeedSquare => "1080×1080. Centre-cropped square.",
            ExportPreset::FeedLandscape => "1080×608. Fit 16:9, no crop.",
            ExportPreset::YouTube1080 => "1920×1080 high-bitrate master, B-frames + aq-mode.",
            ExportPreset::YouTube4K => {
                "Upscaled to 3840×2160 to escape YouTube's 1080p compression tier."
            }
        }
    }

    /// `true` when this preset re-encodes (anything but [`RawMosh`]).
    pub fn re_encodes(self) -> bool {
        !matches!(self, ExportPreset::RawMosh)
    }

    /// Filename-safe tag, used by the "export for all platforms" batch.
    pub fn file_tag(self) -> &'static str {
        match self {
            ExportPreset::RawMosh => "raw",
            ExportPreset::ReelsBlur => "reels_blur",
            ExportPreset::ReelsCrop => "reels_crop",
            ExportPreset::ReelsTriptych => "reels_triptych",
            ExportPreset::FeedSquare => "feed_square",
            ExportPreset::FeedLandscape => "feed_landscape",
            ExportPreset::YouTube1080 => "youtube_1080",
            ExportPreset::YouTube4K => "youtube_4k",
        }
    }

    /// Curated one-per-target set rendered by "Export for all platforms".
    pub const ALL_PLATFORMS: [ExportPreset; 4] = [
        ExportPreset::ReelsBlur,
        ExportPreset::FeedSquare,
        ExportPreset::YouTube1080,
        ExportPreset::YouTube4K,
    ];

    /// The video filter graph for this preset, or `None` for `RawMosh`.
    fn filter(self) -> Option<FilterSpec> {
        // Source is 1920×1080 (16:9). Canvases below are exact and even.
        match self {
            ExportPreset::RawMosh => None,
            // Blurred enlarged background + centred foreground. Multi-node graph.
            ExportPreset::ReelsBlur => Some(FilterSpec::Complex(
                "[0:v]split=2[fg][bg];\
                 [bg]scale=1080:1920:force_original_aspect_ratio=increase,\
                 crop=1080:1920,boxblur=luma_radius=24:luma_power=2,eq=brightness=-0.2[bgo];\
                 [fg]scale=1080:-2[fgo];\
                 [bgo][fgo]overlay=(W-w)/2:(H-h)/2,format=yuv420p[vout]"
                    .into(),
            )),
            // Three stacked copies into a 9:16 canvas.
            ExportPreset::ReelsTriptych => Some(FilterSpec::Complex(
                "[0:v]scale=1080:-2,split=3[c1][c2][c3];\
                 [c1][c2][c3]vstack=inputs=3[stack];\
                 [stack]scale=1080:1920:force_original_aspect_ratio=decrease,\
                 pad=1080:1920:(ow-iw)/2:(oh-ih)/2:black,format=yuv420p[vout]"
                    .into(),
            )),
            // Simple single-chain filters (stay on stream 0:v).
            ExportPreset::ReelsCrop => Some(FilterSpec::Simple(
                "crop=ih*9/16:ih,scale=1080:1920,setsar=1,format=yuv420p".into(),
            )),
            ExportPreset::FeedSquare => Some(FilterSpec::Simple(
                "crop=ih:ih,scale=1080:1080,setsar=1,format=yuv420p".into(),
            )),
            ExportPreset::FeedLandscape => Some(FilterSpec::Simple(
                "scale=1080:608:force_original_aspect_ratio=decrease,\
                 pad=1080:608:(ow-iw)/2:(oh-ih)/2:black,setsar=1,format=yuv420p"
                    .into(),
            )),
            ExportPreset::YouTube1080 => Some(FilterSpec::Simple(
                "scale=1920:1080:force_original_aspect_ratio=decrease,\
                 pad=1920:1080:(ow-iw)/2:(oh-ih)/2:black,setsar=1,format=yuv420p"
                    .into(),
            )),
            ExportPreset::YouTube4K => Some(FilterSpec::Simple(
                "scale=3840:2160:force_original_aspect_ratio=decrease:flags=lanczos,\
                 pad=3840:2160:(ow-iw)/2:(oh-ih)/2:black,setsar=1,format=yuv420p"
                    .into(),
            )),
        }
    }

    /// x264 quality knobs for this preset (empty for `RawMosh`).
    fn video_codec_args(self, fps: u32) -> Vec<String> {
        if !self.re_encodes() {
            return vec!["-c:v".into(), "copy".into()];
        }
        // Closed ~2 s GOP so platform segmenters get predictable cut points.
        let keyint = (fps.max(1) * 2).to_string();
        let mut args = vec![
            "-c:v".into(),
            "libx264".into(),
            "-profile:v".into(),
            "high".into(),
            "-pix_fmt".into(),
            "yuv420p".into(),
            "-g".into(),
            keyint.clone(),
            "-keyint_min".into(),
            keyint,
            "-sc_threshold".into(),
            "0".into(),
        ];
        // The delivery master is never moshed again, so we let x264 use its
        // default B-frames + better rate-control for cleaner compression. We
        // tune per platform: Instagram caps hard, so a CRF master is plenty;
        // YouTube rewards a generous high-bitrate master.
        match self {
            ExportPreset::YouTube1080 => {
                args.extend([
                    "-preset".into(),
                    "slow".into(),
                    "-crf".into(),
                    "16".into(),
                    "-maxrate".into(),
                    "16M".into(),
                    "-bufsize".into(),
                    "32M".into(),
                    "-x264-params".into(),
                    "aq-mode=3".into(),
                    "-colorspace".into(),
                    "bt709".into(),
                    "-color_primaries".into(),
                    "bt709".into(),
                    "-color_trc".into(),
                    "bt709".into(),
                ]);
            }
            ExportPreset::YouTube4K => {
                args.extend([
                    "-preset".into(),
                    "slow".into(),
                    "-crf".into(),
                    "16".into(),
                    "-maxrate".into(),
                    "45M".into(),
                    "-bufsize".into(),
                    "90M".into(),
                    "-x264-params".into(),
                    "aq-mode=3".into(),
                    "-colorspace".into(),
                    "bt709".into(),
                    "-color_primaries".into(),
                    "bt709".into(),
                    "-color_trc".into(),
                    "bt709".into(),
                ]);
            }
            // Instagram presets: medium preset, visually-lossless CRF master.
            _ => {
                args.extend([
                    "-preset".into(),
                    "medium".into(),
                    "-crf".into(),
                    "17".into(),
                    "-colorspace".into(),
                    "bt709".into(),
                    "-color_primaries".into(),
                    "bt709".into(),
                    "-color_trc".into(),
                    "bt709".into(),
                ]);
            }
        }
        args
    }

    /// AAC bitrate handed to the muxer for this preset (kbps).
    fn audio_kbps(self) -> u32 {
        match self {
            ExportPreset::YouTube1080 | ExportPreset::YouTube4K => 384,
            _ => 256,
        }
    }

    /// Build the complete ffmpeg argument vector for the final delivery pass.
    ///
    /// `video_in` is the remuxed moshed video; `audio_in` is the mixed WAV (if
    /// the timeline has audio). The returned args run as
    /// `ffmpeg <args> <output>` (output path is included).
    pub fn build_ffmpeg_args(
        self,
        video_in: &str,
        audio_in: Option<&str>,
        output: &str,
        fps: u32,
    ) -> Vec<String> {
        let mut args = vec!["-y".to_string(), "-i".into(), video_in.to_string()];
        if let Some(a) = audio_in {
            args.push("-i".into());
            args.push(a.to_string());
        }

        // Video stream selection + optional filtering.
        match self.filter() {
            None => {
                // Raw remux: copy video, map streams explicitly when audio present.
                if audio_in.is_some() {
                    args.extend(["-map".into(), "0:v:0".into(), "-map".into(), "1:a:0".into()]);
                }
                args.extend(self.video_codec_args(fps));
            }
            Some(FilterSpec::Simple(vf)) => {
                args.extend(["-vf".into(), vf]);
                if audio_in.is_some() {
                    args.extend(["-map".into(), "0:v:0".into(), "-map".into(), "1:a:0".into()]);
                }
                args.extend(self.video_codec_args(fps));
            }
            Some(FilterSpec::Complex(graph)) => {
                args.extend(["-filter_complex".into(), graph]);
                args.extend(["-map".into(), "[vout]".into()]);
                if audio_in.is_some() {
                    args.extend(["-map".into(), "1:a:0".into()]);
                }
                args.extend(self.video_codec_args(fps));
            }
        }

        // Audio + container flags.
        if audio_in.is_some() {
            args.extend([
                "-c:a".into(),
                "aac".into(),
                "-b:a".into(),
                format!("{}k", self.audio_kbps()),
            ]);
        }
        args.extend(["-movflags".into(), "+faststart".into()]);
        args.push(output.to_string());
        args
    }
}

/// A video filter graph: a single chain that stays on stream `0:v`, or a
/// multi-node graph that must be routed through `-filter_complex` and produces
/// a labelled `[vout]` output.
enum FilterSpec {
    Simple(String),
    Complex(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn joined(p: ExportPreset, audio: bool) -> String {
        p.build_ffmpeg_args("in.mp4", audio.then_some("a.wav"), "out.mp4", 30)
            .join(" ")
    }

    #[test]
    fn raw_mosh_copies_video() {
        let a = joined(ExportPreset::RawMosh, false);
        assert!(a.contains("-c:v copy"));
        assert!(!a.contains("libx264"));
        // Output path is last and faststart is set.
        assert!(a.ends_with("out.mp4"));
        assert!(a.contains("-movflags +faststart"));
    }

    #[test]
    fn raw_mosh_with_audio_maps_streams() {
        let a = joined(ExportPreset::RawMosh, true);
        assert!(a.contains("-map 0:v:0 -map 1:a:0"));
        assert!(a.contains("-c:a aac -b:a 256k"));
    }

    #[test]
    fn reencode_presets_set_closed_2s_gop() {
        // fps=30 → keyint 60.
        for p in [
            ExportPreset::ReelsCrop,
            ExportPreset::FeedSquare,
            ExportPreset::YouTube1080,
            ExportPreset::YouTube4K,
        ] {
            let a = joined(p, false);
            assert!(a.contains("libx264"), "{} should re-encode", p.label());
            assert!(a.contains("-g 60"), "{} GOP", p.label());
            assert!(a.contains("-keyint_min 60"), "{} keyint_min", p.label());
            assert!(a.contains("-sc_threshold 0"), "{} closed GOP", p.label());
        }
    }

    #[test]
    fn simple_filter_presets_use_vf_and_map_video0() {
        let a = joined(ExportPreset::ReelsCrop, true);
        assert!(a.contains("-vf crop=ih*9/16:ih"));
        assert!(a.contains("-map 0:v:0 -map 1:a:0"));
    }

    #[test]
    fn complex_filter_presets_route_through_filter_complex() {
        // Blur + triptych are multi-node graphs.
        for p in [ExportPreset::ReelsBlur, ExportPreset::ReelsTriptych] {
            let a = joined(p, true);
            assert!(a.contains("-filter_complex"), "{} complex", p.label());
            assert!(a.contains("[vout]"), "{} vout label", p.label());
            // Video comes from the graph, audio still from input 1.
            assert!(a.contains("-map [vout] -map 1:a:0"), "{} mapping", p.label());
            assert!(!a.contains("-map 0:v:0"), "{} must not map raw video", p.label());
        }
    }

    #[test]
    fn youtube_master_is_generous() {
        assert!(joined(ExportPreset::YouTube4K, false).contains("3840:2160"));
        assert!(joined(ExportPreset::YouTube4K, false).contains("-maxrate 45M"));
        assert!(joined(ExportPreset::YouTube1080, true).contains("-b:a 384k"));
        assert!(joined(ExportPreset::YouTube1080, false).contains("aq-mode=3"));
    }

    #[test]
    fn all_presets_have_distinct_labels() {
        let mut labels: Vec<&str> = ExportPreset::ALL.iter().map(|p| p.label()).collect();
        labels.sort_unstable();
        labels.dedup();
        assert_eq!(labels.len(), ExportPreset::ALL.len());
    }
}
