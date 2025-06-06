use std::borrow::Cow;
use hound::{WavWriter, WavSpec, SampleFormat};
use crate::error::WhisperStreamError;
use std::fs;
use std::path::Path;
use log::{warn, debug};

/// Pads an audio segment with silence if it's shorter than `min_samples`.
///
/// # Arguments
/// * `audio_segment`: The input audio segment.
/// * `min_samples`: The minimum number of samples the output segment should have.
///
/// # Returns
/// A `Cow<[f32]>` which is either a borrowed slice of the original audio
/// if no padding was needed, or an owned, padded `Vec<f32>`.
pub fn pad_audio_if_needed(audio_segment: &[f32], min_samples: usize) -> Cow<'_, [f32]> {
    if audio_segment.len() < min_samples {
        let mut padded_segment = Vec::with_capacity(min_samples);
        padded_segment.extend_from_slice(audio_segment);
        let padding_needed = min_samples - audio_segment.len();
        padded_segment.extend(std::iter::repeat_n(0.0f32, padding_needed));
        Cow::Owned(padded_segment)
    } else {
        Cow::Borrowed(audio_segment)
    }
}

/// Handles recording audio chunks to a WAV file.
pub struct WavAudioRecorder {
    writer: Option<WavWriter<std::io::BufWriter<std::fs::File>>>,
    path: String,
    is_recording_active: bool,
}

impl WavAudioRecorder {
    /// Creates a new `WavAudioRecorder`.
    ///
    /// # Arguments
    /// * `path_opt`: Optional path to save the WAV file. If `None`, recording is disabled.
    pub fn new(path_opt: Option<&str>) -> Result<Self, WhisperStreamError> {
        match path_opt {
            Some(p) => {
                // Create parent directory if it doesn't exist
                if let Some(parent_dir) = Path::new(p).parent() {
                    if !parent_dir.exists() {
                        fs::create_dir_all(parent_dir).map_err(|e| WhisperStreamError::Io { source: e })?;
                    }
                }

                let spec = WavSpec {
                    channels: 1,        // Whisper processes mono audio
                    sample_rate: 16000, // Whisper processes 16kHz audio
                    bits_per_sample: 16,
                    sample_format: SampleFormat::Int,
                };
                let writer = WavWriter::create(p, spec)
                    .map_err(|e| WhisperStreamError::Hound { source: e })?;
                Ok(Self {
                    writer: Some(writer),
                    path: p.to_string(),
                    is_recording_active: true,
                })
            }
            None => Ok(Self {
                writer: None,
                path: String::new(),
                is_recording_active: false,
            }),
        }
    }

    /// Writes an audio chunk to the WAV file if recording is active.
    ///
    /// # Arguments
    /// * `audio_chunk`: A slice of `f32` audio samples (expected to be mono, 16kHz).
    ///
	/// Samples should be in the range -1.0 to 1.0.
    pub fn write_audio_chunk(&mut self, audio_chunk: &[f32]) -> Result<(), WhisperStreamError> {
        if let Some(writer) = self.writer.as_mut() {
            let mut min_sample = f32::INFINITY;
            let mut max_sample = f32::NEG_INFINITY;
            let mut non_zero_count = 0;

            for &sample_f32_original in audio_chunk {
                min_sample = min_sample.min(sample_f32_original);
                max_sample = max_sample.max(sample_f32_original);
                if sample_f32_original != 0.0 {
                    non_zero_count += 1;
                }

                let sample_f32 = if sample_f32_original.is_finite() {
                    sample_f32_original
                } else {
                    warn!("Non-finite audio sample detected: {}. Replacing with 0.0.", sample_f32_original);
                    0.0
                };

                // Clamp to [-1.0, 1.0) then scale and cast
                let clamped_sample = sample_f32.clamp(-1.0, 1.0 - f32::EPSILON);
                // Scale to i16 range and round to nearest integer
                let scaled = clamped_sample * i16::MAX as f32;
                let sample_i16 = scaled.round() as i16;
                if let Err(e) = writer.write_sample(sample_i16) {
                    return Err(WhisperStreamError::Hound { source: e });
                }
            }

            debug!("[WAV Writer] Chunk stats: len={}, non_zero={}, range=[{:.6}, {:.6}]",
                audio_chunk.len(), non_zero_count, min_sample, max_sample);
        }
        Ok(())
    }

    /// Finalizes the WAV file. Must be called to complete the recording.
    /// Returns a system message indicating the result.
    pub fn finalize(mut self) -> Result<Option<String>, WhisperStreamError> {
        // Use a match statement for clearer logic based on the state.
        // self.writer is taken, so it becomes None after the first call or if initially None.
        match (self.writer.take(), self.is_recording_active, !self.path.is_empty()) {
            (Some(writer), true, true) => {
                // Active recording, valid path, writer exists: finalize and report success.
                writer.finalize().map_err(|e| WhisperStreamError::Hound { source: e })?;
                Ok(Some(format!("[Recording] Finished saving audio to {}", self.path)))
            }
            (Some(writer), _, _) => {
                // Writer existed but state was inconsistent (e.g. not active or no path), still try to finalize.
                // This case helps ensure the file is closed if it was opened.
                writer.finalize().map_err(|e| WhisperStreamError::Hound { source: e })?;
                Ok(Some(format!("[Recording] Finalized audio file at {} (state was potentially inconsistent).", self.path)))
            }
            (None, true, true) => {
                // Was supposed to be recording with a valid path, but writer is gone (e.g., finalize called twice or error during creation).
                Ok(Some(format!("[Recording] Attempted to finalize, but no active writer for {}. File might have been finalized or failed to open.", self.path)))
            }
            (None, true, false) => {
                // Was supposed to be recording, but no path and no writer.
                Ok(Some("[Recording] Recording was intended but path was empty and no writer; nothing saved.".to_string()))
            }
            (None, false, _) => {
                // Not recording, or already finalized. No action needed, no message.
                Ok(None)
            }
        }
    }

    pub fn is_recording(&self) -> bool {
        self.is_recording_active
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;

    #[test]
    fn test_pad_audio_if_needed_no_padding() {
        let input = vec![0.1, 0.2, 0.3];
        let result = pad_audio_if_needed(&input, 3);
        assert_eq!(&*result, &[0.1, 0.2, 0.3]);
        assert!(matches!(result, std::borrow::Cow::Borrowed(_)));
    }

    #[test]
    fn test_pad_audio_if_needed_with_padding() {
        let input = vec![0.1, 0.2];
        let result = pad_audio_if_needed(&input, 4);
        assert_eq!(&*result, &[0.1, 0.2, 0.0, 0.0]);
        assert!(matches!(result, std::borrow::Cow::Owned(_)));
    }

    #[test]
    fn test_wav_audio_recorder_write_and_finalize() {
        let test_path = "test_output.wav";
        // Clean up before test
        let _ = fs::remove_file(test_path);
        let mut recorder = WavAudioRecorder::new(Some(test_path)).expect("Failed to create recorder");
        assert!(recorder.is_recording());
        let audio_chunk = vec![0.0f32, 0.5, -0.5, 1.0, -1.0];
        recorder.write_audio_chunk(&audio_chunk).expect("Failed to write chunk");
        let msg = recorder.finalize().expect("Failed to finalize");
        assert!(msg.is_some());
        assert!(Path::new(test_path).exists());
        // Clean up after test
        let _ = fs::remove_file(test_path);
    }

    #[test]
    fn test_wav_audio_recorder_no_path() {
        let recorder = WavAudioRecorder::new(None).expect("Failed to create recorder");
        assert!(!recorder.is_recording());
    }
}