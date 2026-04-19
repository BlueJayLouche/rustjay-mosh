use std::path::Path;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum AudioError {
    #[error("ffmpeg transcoding failed")]
    TranscodeFailed,
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
    #[error("wav write error: {0}")]
    Wav(#[from] hound::Error),
}

#[derive(Debug, Clone, Copy)]
pub struct Peak {
    pub min: f32,
    pub max: f32,
}

#[derive(Debug, Clone)]
pub struct AudioClip {
    pub name: String,
    /// Interleaved f32 samples.
    pub samples: Vec<f32>,
    pub sample_rate: u32,
    pub channels: usize,
    /// One peak entry per video frame (at project fps).
    pub peaks: Vec<Peak>,
}

#[derive(Debug, Clone)]
pub struct AudioTimelineClip {
    pub audio_clip_idx: usize,
    pub start_frame: i64,
    pub frame_count: usize,
    pub source_offset: usize,
    pub fade_in_frames: usize,
    pub fade_out_frames: usize,
    pub selected: bool,
}

impl AudioTimelineClip {
    pub fn end_frame(&self) -> i64 {
        self.start_frame + self.frame_count as i64
    }
}

/// Decode an audio file to interleaved f32 samples using ffmpeg.
/// `project_fps` is used to precompute per-frame peaks.
pub fn import_audio(
    path: &Path,
    name: impl Into<String>,
    project_fps: u32,
) -> Result<AudioClip, AudioError> {
    let temp_dir = tempfile::tempdir()?;
    let temp_path = temp_dir.path().join("audio.raw");

    let status = std::process::Command::new("ffmpeg")
        .args([
            "-y",
            "-i", path.to_str().unwrap_or(""),
            "-vn",
            "-ar", "48000",
            "-ac", "2",
            "-f", "f32le",
            "-acodec", "pcm_f32le",
            temp_path.to_str().unwrap_or(""),
        ])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map_err(|_| AudioError::TranscodeFailed)?;

    if !status.success() {
        return Err(AudioError::TranscodeFailed);
    }

    let bytes = std::fs::read(&temp_path)?;
    let sample_count = bytes.len() / 4;
    let mut samples = Vec::with_capacity(sample_count);
    for chunk in bytes.chunks_exact(4) {
        samples.push(f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
    }

    let channels = 2usize;
    let sample_rate = 48000u32;
    let peaks = compute_peaks(&samples, sample_rate, channels, project_fps);

    Ok(AudioClip {
        name: name.into(),
        samples,
        sample_rate,
        channels,
        peaks,
    })
}

/// Compute min/max peak per video frame.
fn compute_peaks(samples: &[f32], sample_rate: u32, channels: usize, fps: u32) -> Vec<Peak> {
    let samples_per_frame = (sample_rate as usize * channels) / fps as usize;
    if samples_per_frame == 0 {
        return vec![];
    }
    let frame_count = samples.len() / samples_per_frame;
    let mut peaks = Vec::with_capacity(frame_count);
    for frame in 0..frame_count {
        let start = frame * samples_per_frame;
        let end = ((frame + 1) * samples_per_frame).min(samples.len());
        let mut min = 0.0f32;
        let mut max = 0.0f32;
        for &s in &samples[start..end] {
            min = min.min(s);
            max = max.max(s);
        }
        peaks.push(Peak { min, max });
    }
    peaks
}

/// Render the mixed audio timeline to a 48 kHz stereo WAV file.
pub fn render_audio_mix(
    audio_clips: &[AudioClip],
    timeline_clips: &[AudioTimelineClip],
    total_frames: usize,
    fps: u32,
    output_wav: &Path,
) -> Result<(), AudioError> {
    let sample_rate = 48000u32;
    let channels = 2usize;
    let total_samples = (total_frames as u64 * sample_rate as u64 / fps as u64) as usize;
    let mut mix = vec![0.0f32; total_samples * channels];

    let mut sorted: Vec<_> = timeline_clips.iter().collect();
    sorted.sort_by_key(|c| c.start_frame);

    for (i, tl) in sorted.iter().enumerate() {
        let clip = &audio_clips[tl.audio_clip_idx];
        let sample_start = (tl.start_frame as u64 * sample_rate as u64 / fps as u64) as usize * channels;
        let source_sample_start = (tl.source_offset as u64 * sample_rate as u64 / fps as u64) as usize * channels;
        let source_sample_count = (tl.frame_count as u64 * sample_rate as u64 / fps as u64) as usize * channels;
        let source_end = (source_sample_start + source_sample_count).min(clip.samples.len());
        let actual_samples = source_end.saturating_sub(source_sample_start);
        if actual_samples == 0 || sample_start >= mix.len() {
            continue;
        }

        let end = (sample_start + actual_samples).min(mix.len());
        let frame_samples = sample_rate as usize * channels / fps as usize;
        let fade_in_samples = tl.fade_in_frames * frame_samples;
        let fade_out_samples = tl.fade_out_frames * frame_samples;

        // Determine crossfade overlap with previous clip.
        let mut crossfade_start_samples = 0usize;
        let _crossfade_end_samples = 0usize;
        if i > 0 {
            let prev = sorted[i - 1];
            if prev.end_frame() == tl.start_frame {
                let _prev_clip = &audio_clips[prev.audio_clip_idx];
                let prev_fade_out_samples = prev.fade_out_frames * frame_samples;
                let cur_fade_in_samples = tl.fade_in_frames * frame_samples;
                let overlap = prev_fade_out_samples.min(cur_fade_in_samples);
                // We only need to know the overlap length; the previous clip's
                // fade-out was already applied above. We just adjust the current
                // clip's fade-in so it ramps from 0..1 across the overlap.
                crossfade_start_samples = overlap;
            }
        }

        for (write_idx, src_idx) in (sample_start..end).zip(source_sample_start..source_end) {
            let local = write_idx - sample_start;
            let gain = if local < crossfade_start_samples {
                // Inside crossfade overlap: linear ramp from 0 to 1
                local as f32 / crossfade_start_samples.max(1) as f32
            } else if local < fade_in_samples {
                (local - crossfade_start_samples) as f32
                    / (fade_in_samples - crossfade_start_samples).max(1) as f32
            } else if local + fade_out_samples >= actual_samples {
                (actual_samples - local - 1) as f32 / fade_out_samples.max(1) as f32
            } else {
                1.0
            };
            mix[write_idx] = (mix[write_idx] + clip.samples[src_idx] * gain).clamp(-1.0, 1.0);
        }
    }

    // Write 48 kHz stereo 32-bit float WAV.
    let spec = hound::WavSpec {
        channels: channels as u16,
        sample_rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut writer = hound::WavWriter::create(output_wav, spec)?;
    for s in mix {
        writer.write_sample(s)?;
    }
    writer.finalize()?;
    Ok(())
}
