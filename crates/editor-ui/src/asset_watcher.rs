//! I-24: filesystem watcher that bridges `notify` events into the
//! editor's frame loop.
//!
//! `notify`'s callback fires on a background thread; the editor is a
//! single-threaded egui loop. We bridge the two via an `mpsc` channel
//! and drain it once per frame from `update()`. That way hot-reload
//! logic runs on the same thread as scene mutation, which keeps the
//! command stack, the asset list, and the viewport rebuild trivially
//! consistent.
//!
//! Design choices:
//!  - Events are *not* fine-grained (created vs. modified vs. removed).
//!    The app's reaction to any of those is the same: re-scan the
//!    asset tree from disk. Tracking kinds would be dead weight.
//!  - Paths are collapsed: if the watcher sees twenty events in one
//!    frame (tool saving a file touches inode, updates mtime, writes
//!    data), `drain()` returns a deduplicated set so the app only
//!    rescans once per unique path.
//!  - The watcher is self-owned: dropping `AssetWatcher` tears down
//!    the `notify::RecommendedWatcher` automatically.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};

use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum WatcherError {
    #[error("failed to initialise filesystem watcher")]
    Init(#[source] notify::Error),
    #[error("failed to watch `{path}`")]
    Watch {
        path:   String,
        #[source]
        source: notify::Error,
    },
}

/// A single filesystem change the editor cares about. Kind is
/// intentionally boolean-ish — the caller's response to all three
/// (Created / Modified / Removed) is "rescan", so preserving the
/// distinction would just be noise.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AssetChangeEvent {
    pub path: PathBuf,
}

pub struct AssetWatcher {
    /// Kept alive so `notify` keeps firing. Field order matters — the
    /// watcher must drop before the receiver so the callback doesn't
    /// fire into a dead channel.
    _watcher: RecommendedWatcher,
    rx:       Receiver<AssetChangeEvent>,
    root:     PathBuf,
}

impl AssetWatcher {
    /// Start watching `root` (recursively). Returns a handle the app
    /// polls each frame via [`drain`](Self::drain).
    pub fn new(root: PathBuf) -> Result<Self, WatcherError> {
        let (tx, rx) = mpsc::channel();
        let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            let Ok(event) = res else {
                return;
            };
            // Filter to events that actually change the asset graph.
            // `Access` alone (read-only file opens) is noise; ignore.
            let relevant = matches!(
                event.kind,
                EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
            );
            if !relevant {
                return;
            }
            for path in event.paths {
                // `.meta` files are editor bookkeeping — skip them so
                // the rescan loop doesn't chase its own writes.
                if path.extension().is_some_and(|ext| ext == "meta") {
                    continue;
                }
                let _ = tx.send(AssetChangeEvent { path });
            }
        })
        .map_err(WatcherError::Init)?;

        watcher
            .watch(&root, RecursiveMode::Recursive)
            .map_err(|source| WatcherError::Watch {
                path: root.display().to_string(),
                source,
            })?;

        Ok(Self {
            _watcher: watcher,
            rx,
            root,
        })
    }

    /// Return the directory this watcher is rooted at. Useful for
    /// logging + tests.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Drain every event pending on the channel, deduplicating paths.
    /// Safe to call every frame — returns an empty vec on idle frames.
    pub fn drain(&self) -> Vec<AssetChangeEvent> {
        let mut seen: HashSet<PathBuf> = HashSet::new();
        let mut out = Vec::new();
        while let Ok(event) = self.rx.try_recv() {
            if seen.insert(event.path.clone()) {
                out.push(event);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::thread;
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    use super::AssetWatcher;

    fn unique_dir(tag: &str) -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("rustforge_watcher_{tag}_{nanos}"))
    }

    #[test]
    fn drain_is_empty_on_idle_watcher() {
        let dir = unique_dir("idle");
        fs::create_dir_all(&dir).unwrap();
        let watcher = AssetWatcher::new(dir.clone()).unwrap();
        // No FS activity — drain must return zero events.
        assert!(watcher.drain().is_empty());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn watcher_observes_file_creation() {
        // notify fires on a backend thread; the test writes a file and
        // polls `drain` until the create event lands or we hit the
        // deadline. On CI this typically resolves within ~10-50ms;
        // local SSDs see sub-ms. 2 seconds is comfortably loose.
        let dir = unique_dir("create");
        fs::create_dir_all(&dir).unwrap();
        let watcher = AssetWatcher::new(dir.clone()).unwrap();

        // Some platforms' watchers need a beat after `watch()` before
        // they'll catch the very next event; yield briefly to let the
        // backend thread register.
        thread::sleep(Duration::from_millis(50));

        let target = dir.join("fresh.ron");
        fs::write(&target, b"(value: 1)").unwrap();

        let deadline = Instant::now() + Duration::from_secs(2);
        let mut observed = false;
        while Instant::now() < deadline {
            let events = watcher.drain();
            if events.iter().any(|event| event.path == target) {
                observed = true;
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }

        assert!(
            observed,
            "watcher failed to observe file creation within 2s"
        );

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn drain_deduplicates_same_path_events() {
        // A tool saving a single file may trigger multiple raw notify
        // events (Create → Modify → Modify). `drain` collapses these
        // per-path so the rescan logic fires once, not thrice.
        let dir = unique_dir("dedup");
        fs::create_dir_all(&dir).unwrap();
        let watcher = AssetWatcher::new(dir.clone()).unwrap();
        thread::sleep(Duration::from_millis(50));

        let target = dir.join("churn.ron");
        for i in 0..5 {
            fs::write(&target, format!("(value: {i})")).unwrap();
        }

        let deadline = Instant::now() + Duration::from_secs(2);
        let mut unique_paths = 0usize;
        while Instant::now() < deadline {
            let events = watcher.drain();
            let matches: Vec<_> =
                events.iter().filter(|e| e.path == target).collect();
            if !matches.is_empty() {
                unique_paths = matches.len();
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }

        // Whatever the backend produced, `drain` must collapse to at
        // most one entry per unique path per call.
        assert!(
            unique_paths <= 1,
            "drain returned {unique_paths} events for the same path"
        );

        fs::remove_dir_all(dir).unwrap();
    }
}
