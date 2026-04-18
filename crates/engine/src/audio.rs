//! Audio data model — I-29.
//!
//! This module is deliberately **data-only**. No rodio, no cpal, no
//! OutputStream — those live in the editor crate where GPU/OS handles
//! already coexist. Keeping `core` audio-agnostic means tests run
//! without an audio device (CI has none) and means a future WASM
//! target can substitute Web Audio at the editor layer without
//! touching the ECS contract.
//!
//! Three types carry the contract:
//!   - [`AudioClipHandle`] — stable `u64` derived from a relative
//!     asset path. Mirrors the `MeshHandle`/`MeshSource` split so
//!     the same "handle for dispatch, path for load" pattern is
//!     consistent across asset kinds.
//!   - [`AudioSource`] — ECS component authored in scene files.
//!     Carries playback parameters (volume, pitch, looping) plus an
//!     `autoplay` flag that makes it fire once when the scene enters
//!     Play mode.
//!   - [`AudioCommand`] — what the audio engine consumes each frame.
//!     Constructed by `World::drain_audio_commands`; lets the core
//!     run headless tests against the "what would play?" output
//!     without actually needing an audio backend.
//!
//! The scene-file shape (authoring):
//!
//! ```ron
//! (
//!     type_name: "AudioSource",
//!     fields: {
//!         "source":   String("audio/impact.wav"),
//!         "volume":   F64(0.8),     // 0..∞, default 1.0
//!         "pitch":    F64(1.0),     // default 1.0
//!         "looping":  Bool(false),  // default false
//!         "autoplay": Bool(true),   // default false
//!     },
//! )
//! ```

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

/// Opaque stable id for an audio clip. Derived from the relative asset
/// path (same scheme as [`crate::world::mesh_handle_for_source`]) so
/// re-loads of the same scene re-use cached decoded samples instead of
/// re-decoding off disk every time.
///
/// `0` is intentionally unreserved: unlike `MeshHandle::UNIT_CUBE`
/// there's no "built-in" audio clip, so we don't need a guard bit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AudioClipHandle(pub u64);

/// Deterministic path → [`AudioClipHandle`] hash. Stability across
/// process restarts is what matters: a scene saved on Monday and
/// loaded on Tuesday must produce the same handle so the editor's
/// decoded-sample cache hits.
pub fn audio_handle_for_source(source: &str) -> AudioClipHandle {
    let mut hasher = DefaultHasher::new();
    source.hash(&mut hasher);
    AudioClipHandle(hasher.finish())
}

/// ECS component authored by scene files. Present on entities that
/// should produce sound; the engine's audio system reads playback
/// parameters from here and routes them to rodio sinks.
#[derive(Debug, Clone, PartialEq)]
pub struct AudioSource {
    /// Relative path into the project's asset tree (e.g.
    /// `"audio/impact.wav"`). Resolution to a filesystem path is the
    /// editor bridge's job, same as [`crate::world::MeshSource`].
    pub path:    String,
    /// Stable handle hashed from `path`. Cached on the component so
    /// the engine doesn't re-hash every frame.
    pub handle:  AudioClipHandle,
    /// 0.0 = silent, 1.0 = unit gain. Values >1 boost but may clip
    /// depending on the backend's mixer. Default 1.0.
    pub volume:  f32,
    /// Playback-rate multiplier. 1.0 = file's native pitch, 2.0 =
    /// one octave up, 0.5 = one octave down. Default 1.0.
    pub pitch:   f32,
    /// Loop forever while the sink is alive. Default false.
    pub looping: bool,
    /// Fire the clip once when Play mode enters. Edit-mode ticks
    /// never trigger autoplay — the editor would become unusable if
    /// clicking around constantly restarted audio.
    pub autoplay: bool,
}

impl AudioSource {
    pub fn new(path: impl Into<String>) -> Self {
        let path = path.into();
        let handle = audio_handle_for_source(&path);
        Self {
            path,
            handle,
            volume:   1.0,
            pitch:    1.0,
            looping:  false,
            autoplay: false,
        }
    }

    pub fn with_volume(mut self, volume: f32) -> Self {
        self.volume = volume.max(0.0);
        self
    }

    pub fn with_pitch(mut self, pitch: f32) -> Self {
        // Zero or negative pitch would silence or reverse — neither
        // is a sensible default. Clamp to a small positive floor.
        self.pitch = pitch.max(0.01);
        self
    }

    pub fn with_looping(mut self, looping: bool) -> Self {
        self.looping = looping;
        self
    }

    pub fn with_autoplay(mut self, autoplay: bool) -> Self {
        self.autoplay = autoplay;
        self
    }
}

/// A single instruction for the audio engine. Enumerated so the engine
/// can round-trip simple patterns (play this, stop that) without a
/// bespoke API per use-case.
///
/// The core drains these into a `Vec` each frame; the editor-side
/// engine maps them to rodio sinks. Decoupling via commands also makes
/// the mixing behaviour scriptable — a future replay system can record
/// the command stream and play it back deterministically.
#[derive(Debug, Clone, PartialEq)]
pub enum AudioCommand {
    /// Start a one-shot (or looping, if the source's flag is set)
    /// playback instance for the given clip. Volume/pitch snapshot
    /// the `AudioSource` at the moment the command was issued.
    Play {
        handle:  AudioClipHandle,
        /// Path the engine resolves to a decoded sample. Carried
        /// alongside the handle so the engine can load on-demand if
        /// the handle isn't resident yet.
        path:    String,
        volume:  f32,
        pitch:   f32,
        looping: bool,
    },
    /// Stop every currently-playing instance of `handle`. No-op if
    /// none are active. Handy for "boss music" transitions — stop
    /// the loop when the player dies.
    StopAll {
        handle: AudioClipHandle,
    },
}

/// Parse an `AudioSource` scene component into the runtime component.
/// Every field is optional except `source`; a component with no
/// `source` (or an empty one) is ignored, since there's nothing to
/// play. Volume/pitch sanitize to non-negative / strictly-positive
/// values so an authored typo can't cause a backend panic.
pub fn extract_audio_source(entity: &crate::scene::SceneEntity) -> Option<AudioSource> {
    use crate::scene::PrimitiveValue;
    let component = entity
        .components
        .iter()
        .find(|c| c.type_name == "AudioSource")?;
    let path = match component.fields.get("source") {
        Some(PrimitiveValue::String(s)) if !s.is_empty() => s.clone(),
        _ => return None,
    };
    let mut source = AudioSource::new(path);
    if let Some(PrimitiveValue::F64(v)) = component.fields.get("volume") {
        source.volume = (*v as f32).max(0.0);
    }
    if let Some(PrimitiveValue::F64(v)) = component.fields.get("pitch") {
        source.pitch = (*v as f32).max(0.01);
    }
    if let Some(PrimitiveValue::Bool(v)) = component.fields.get("looping") {
        source.looping = *v;
    }
    if let Some(PrimitiveValue::Bool(v)) = component.fields.get("autoplay") {
        source.autoplay = *v;
    }
    Some(source)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::{ComponentData, PrimitiveValue, SceneEntity, SceneId};

    #[test]
    fn audio_handle_is_stable_across_calls() {
        let a = audio_handle_for_source("audio/impact.wav");
        let b = audio_handle_for_source("audio/impact.wav");
        let c = audio_handle_for_source("audio/footstep.wav");
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn new_audio_source_caches_handle() {
        let src = AudioSource::new("audio/impact.wav");
        assert_eq!(src.handle, audio_handle_for_source("audio/impact.wav"));
        assert_eq!(src.volume, 1.0);
        assert_eq!(src.pitch, 1.0);
        assert!(!src.looping);
        assert!(!src.autoplay);
    }

    #[test]
    fn with_volume_clamps_negative_to_zero() {
        let src = AudioSource::new("x.wav").with_volume(-0.5);
        assert_eq!(src.volume, 0.0);
    }

    #[test]
    fn with_pitch_clamps_zero_to_floor() {
        let src = AudioSource::new("x.wav").with_pitch(0.0);
        assert!(src.pitch >= 0.01);
    }

    #[test]
    fn extract_audio_source_honors_every_field() {
        let entity = SceneEntity::new(SceneId::new(1), "Boombox").with_component(
            ComponentData::new("AudioSource")
                .with_field("source", PrimitiveValue::String("audio/loop.ogg".into()))
                .with_field("volume", PrimitiveValue::F64(0.4))
                .with_field("pitch", PrimitiveValue::F64(1.25))
                .with_field("looping", PrimitiveValue::Bool(true))
                .with_field("autoplay", PrimitiveValue::Bool(true)),
        );
        let src = extract_audio_source(&entity).unwrap();
        assert_eq!(src.path, "audio/loop.ogg");
        assert!((src.volume - 0.4).abs() < 1e-6);
        assert!((src.pitch - 1.25).abs() < 1e-6);
        assert!(src.looping);
        assert!(src.autoplay);
    }

    #[test]
    fn extract_audio_source_returns_none_without_source_field() {
        let entity = SceneEntity::new(SceneId::new(1), "Silent")
            .with_component(ComponentData::new("AudioSource"));
        assert!(extract_audio_source(&entity).is_none());
    }
}
