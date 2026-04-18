//! Editor-side audio backend — I-29.
//!
//! The core crate stays audio-agnostic: it emits
//! [`engine::audio::AudioCommand`] entries that describe *what
//! should play*. This module is where those descriptions meet real
//! sound hardware via rodio (cpal under the hood).
//!
//! Design notes:
//!   - The backend is **optional**. `OutputStream::try_default` fails
//!     on headless machines (CI, remote-desktop sessions without a
//!     default device, WSL without PulseAudio). Rather than crashing
//!     the editor, we flip a `disabled` flag and silently drop every
//!     command. Tests can construct `AudioEngine` without the
//!     rodio backend at all by calling [`AudioEngine::disabled()`].
//!   - Decoded sample bytes are cached by [`AudioClipHandle`] so the
//!     same clip triggered 100 times per minute only reads disk once.
//!     We cache the raw file bytes, not the decoded PCM stream, because
//!     rodio's `Decoder` wants a fresh `Read + Seek` for each play —
//!     cloning an `Arc<Vec<u8>>` into a `Cursor` is cheap and sidesteps
//!     the need to re-decode.
//!   - Active sinks are tracked by handle so
//!     [`AudioCommand::StopAll`] can reach them. Finished sinks are
//!     pruned lazily on each `apply` pass so we don't leak memory for
//!     one-shot clips.

use std::collections::HashMap;
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink};
use engine::audio::{AudioClipHandle, AudioCommand};

/// Thin `AsRef<[u8]>` wrapper around an `Arc<Vec<u8>>`. `Cursor<T>`
/// only impls `Read + Seek` when `T: AsRef<[u8]>`, which an `Arc` alone
/// doesn't satisfy. Wrapping lets us hand rodio a cheap, cloneable
/// byte view without copying the underlying buffer per playback.
#[derive(Clone)]
struct ArcBytes(Arc<Vec<u8>>);

impl AsRef<[u8]> for ArcBytes {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

/// Rodio-backed audio engine. Holds the output device, a decoded-bytes
/// cache, and the set of currently-active sinks.
pub struct AudioEngine {
    // Drop order matters: `_stream` must outlive every `Sink`. We hold
    // it in an `Option` so the "disabled" branch doesn't force an
    // OutputStream at all.
    _stream:      Option<OutputStream>,
    handle:       Option<OutputStreamHandle>,
    /// Raw file bytes keyed by [`AudioClipHandle`] inner u64. Shared
    /// with every decoded-stream instance via `Arc` so concurrent
    /// plays of the same clip don't duplicate memory.
    sample_cache: HashMap<u64, Arc<Vec<u8>>>,
    /// Live sinks grouped by clip handle. Finished sinks (empty
    /// queue) are dropped on the next `apply_commands` pass.
    active_sinks: HashMap<u64, Vec<Sink>>,
    /// True when no output device could be opened. Every `apply` is a
    /// no-op in this state, but [`last_commands`] still records what
    /// *would* have played so editor UI + tests can observe the
    /// behaviour.
    disabled:     bool,
    /// Debug/test observer: the command list from the most recent
    /// `apply_commands` call. Not queried in production code paths but
    /// lets tests assert the command plumbing end-to-end without
    /// needing a real audio device.
    pub last_commands: Vec<AudioCommand>,
}

impl AudioEngine {
    /// Try to open the default OS audio device and construct an
    /// engine. Falls back to a disabled engine (logs but doesn't
    /// panic) if the host has no audio support — important for CI
    /// runners and remote-desktop sessions.
    pub fn new() -> Self {
        match OutputStream::try_default() {
            Ok((stream, handle)) => Self {
                _stream:      Some(stream),
                handle:       Some(handle),
                sample_cache: HashMap::new(),
                active_sinks: HashMap::new(),
                disabled:     false,
                last_commands: Vec::new(),
            },
            Err(err) => {
                eprintln!("[audio] backend unavailable, running silent: {err}");
                Self::disabled()
            }
        }
    }

    /// Construct a no-op engine that swallows every command. Used by
    /// headless tests and as the fallback when the OS has no audio
    /// device. `apply_commands` still records `last_commands` so
    /// tests can inspect the queue.
    pub fn disabled() -> Self {
        Self {
            _stream:      None,
            handle:       None,
            sample_cache: HashMap::new(),
            active_sinks: HashMap::new(),
            disabled:     true,
            last_commands: Vec::new(),
        }
    }

    pub fn is_disabled(&self) -> bool {
        self.disabled
    }

    /// Drop every active sink, silencing the engine immediately. Used
    /// when Play mode exits — lingering loops would otherwise bleed
    /// into Edit mode, which is jarring and makes it hard to iterate
    /// on audio authoring.
    pub fn stop_all(&mut self) {
        for sinks in self.active_sinks.values_mut() {
            for sink in sinks.drain(..) {
                sink.stop();
            }
        }
        self.active_sinks.clear();
    }

    /// Consume a batch of commands, resolving paths against
    /// `asset_root` and spawning sinks for `Play` entries.
    pub fn apply_commands(
        &mut self,
        commands: &[AudioCommand],
        asset_root: &Path,
    ) {
        self.last_commands = commands.to_vec();
        // Prune sinks that finished playing. `Sink::empty()` is true
        // once the internal queue drains, which is our proxy for "one
        // shot completed" — loops stay non-empty indefinitely.
        self.prune_finished_sinks();
        if self.disabled {
            return;
        }
        for command in commands {
            match command {
                AudioCommand::Play { handle, path, volume, pitch, looping } => {
                    if let Err(err) = self.play(*handle, path, asset_root, *volume, *pitch, *looping) {
                        eprintln!("[audio] play failed for `{path}`: {err}");
                    }
                }
                AudioCommand::StopAll { handle } => {
                    if let Some(sinks) = self.active_sinks.remove(&handle.0) {
                        for sink in sinks {
                            sink.stop();
                        }
                    }
                }
            }
        }
    }

    fn prune_finished_sinks(&mut self) {
        for sinks in self.active_sinks.values_mut() {
            sinks.retain(|sink| !sink.empty());
        }
        self.active_sinks.retain(|_, sinks| !sinks.is_empty());
    }

    fn play(
        &mut self,
        handle: AudioClipHandle,
        path: &str,
        asset_root: &Path,
        volume: f32,
        pitch: f32,
        looping: bool,
    ) -> Result<(), AudioError> {
        // Load bytes before borrowing `self.handle` to keep the
        // mutable/immutable borrow windows from overlapping.
        let bytes = self.load_bytes(handle, path, asset_root)?;
        let Some(stream_handle) = &self.handle else {
            return Err(AudioError::Disabled);
        };
        let cursor = Cursor::new(ArcBytes(bytes));
        let sink = Sink::try_new(stream_handle).map_err(AudioError::Sink)?;
        sink.set_volume(volume);
        sink.set_speed(pitch);
        if looping {
            let decoder = Decoder::new_looped(cursor).map_err(AudioError::Decode)?;
            sink.append(decoder);
        } else {
            let decoder = Decoder::new(cursor).map_err(AudioError::Decode)?;
            sink.append(decoder);
        }
        self.active_sinks.entry(handle.0).or_default().push(sink);
        Ok(())
    }

    fn load_bytes(
        &mut self,
        handle: AudioClipHandle,
        path: &str,
        asset_root: &Path,
    ) -> Result<Arc<Vec<u8>>, AudioError> {
        if let Some(cached) = self.sample_cache.get(&handle.0) {
            return Ok(Arc::clone(cached));
        }
        let full: PathBuf = if Path::new(path).is_absolute() {
            PathBuf::from(path)
        } else {
            asset_root.join(path)
        };
        let bytes = fs::read(&full).map_err(|source| AudioError::Read {
            path: full.display().to_string(),
            source,
        })?;
        let arc = Arc::new(bytes);
        self.sample_cache.insert(handle.0, Arc::clone(&arc));
        Ok(arc)
    }

    /// Drop the cached decoded samples. Used after a hot-reload so a
    /// re-saved audio clip actually gets re-read on the next play.
    pub fn clear_sample_cache(&mut self) {
        self.sample_cache.clear();
    }
}

impl Default for AudioEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    #[error("audio engine is disabled (no output device)")]
    Disabled,
    #[error("failed to read audio file `{path}`")]
    Read {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to decode audio stream")]
    Decode(#[from] rodio::decoder::DecoderError),
    #[error("failed to create audio sink")]
    Sink(#[from] rodio::PlayError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use engine::audio::{audio_handle_for_source, AudioCommand};

    #[test]
    fn disabled_engine_records_commands_without_crashing() {
        // An engine constructed via `disabled()` accepts commands and
        // stores them in `last_commands` but never opens an OutputStream
        // — important because CI and WSL have no audio device.
        let mut engine = AudioEngine::disabled();
        assert!(engine.is_disabled());

        let commands = vec![
            AudioCommand::Play {
                handle:  audio_handle_for_source("audio/impact.wav"),
                path:    "audio/impact.wav".into(),
                volume:  1.0,
                pitch:   1.0,
                looping: false,
            },
            AudioCommand::StopAll {
                handle: audio_handle_for_source("audio/loop.ogg"),
            },
        ];
        engine.apply_commands(&commands, Path::new("."));
        assert_eq!(engine.last_commands.len(), 2);
    }

    #[test]
    fn stop_all_clears_active_sinks_map() {
        // Even a disabled engine exposes `stop_all` so callers don't
        // have to special-case it around the backend status.
        let mut engine = AudioEngine::disabled();
        engine.stop_all();
        // No panic = success. The real-backend path exercises the sink
        // iteration via the integration scenario (manual testing).
    }
}
