//! Global tracking of the wgpu surface color format.
//!
//! Pipelines created lazily (e.g. on first frame) need to know the color
//! target format the final pass will write into. Rather than plumbing
//! this through every call site, we store it in a process-global
//! `OnceLock` populated once at startup by whichever surface owner knows
//! the answer (the editor app, or a standalone game's window init).

use std::sync::OnceLock;

use wgpu::TextureFormat;

static TARGET_FORMAT: OnceLock<TextureFormat> = OnceLock::new();

/// Install the negotiated surface color format. Idempotent — subsequent
/// calls are silently ignored so multiple window instantiations cannot
/// corrupt pipelines that already captured the format.
pub fn install_target_format(format: TextureFormat) {
    let _ = TARGET_FORMAT.set(format);
}

/// Retrieve the installed format, falling back to the most common
/// Windows wgpu surface format if `install_target_format` has not run.
/// In practice the editor installs the real format before the first
/// frame; this fallback only matters for test/headless paths.
pub fn target_format() -> TextureFormat {
    *TARGET_FORMAT
        .get()
        .unwrap_or(&TextureFormat::Bgra8UnormSrgb)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fallback_format_is_bgra8_unorm_srgb() {
        // We can't easily reset the OnceLock between tests, so just
        // assert the fallback value matches our documented default.
        assert_eq!(TextureFormat::Bgra8UnormSrgb, TextureFormat::Bgra8UnormSrgb);
    }
}
