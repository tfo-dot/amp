#![allow(dead_code)]
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime};

pub struct ExtensionWatcher {
    running: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl ExtensionWatcher {
    pub fn new() -> Self {
        Self {
            running: Arc::new(AtomicBool::new(false)),
            handle: None,
        }
    }

    /// Starts a background thread watching the given directory or default parts directory,
    /// sending changed script paths over the provided channel.
    pub fn start<F>(&mut self, watch_dirs: Vec<PathBuf>, on_change: F)
    where
        F: Fn(String) + Send + Sync + 'static,
    {
        if self.running.load(Ordering::SeqCst) {
            return;
        }

        self.running.store(true, Ordering::SeqCst);
        let running = self.running.clone();
        let on_change = Arc::new(on_change);

        let handle = thread::spawn(move || {
            let mut file_mtimes: HashMap<PathBuf, SystemTime> = HashMap::new();

            // Initial scan
            for dir in &watch_dirs {
                if let Ok(entries) = std::fs::read_dir(dir) {
                    for entry in entries.flatten() {
                        let path = entry.path();
                        if path.is_file() && path.extension().and_then(|s| s.to_str()) == Some("pts") {
                            if let Ok(meta) = std::fs::metadata(&path) {
                                if let Ok(mtime) = meta.modified() {
                                    file_mtimes.insert(path, mtime);
                                }
                            }
                        }
                    }
                }
            }

            while running.load(Ordering::SeqCst) {
                thread::sleep(Duration::from_millis(800));

                for dir in &watch_dirs {
                    if let Ok(entries) = std::fs::read_dir(dir) {
                        for entry in entries.flatten() {
                            let path = entry.path();
                            if path.is_file() && path.extension().and_then(|s| s.to_str()) == Some("pts") {
                                if let Ok(meta) = std::fs::metadata(&path) {
                                    if let Ok(mtime) = meta.modified() {
                                        let is_changed = match file_mtimes.get(&path) {
                                            Some(&prev_time) => mtime > prev_time,
                                            None => true,
                                        };

                                        if is_changed {
                                            file_mtimes.insert(path.clone(), mtime);
                                            let path_str = path.to_string_lossy().to_string();
                                            eprintln!("[ExtensionWatcher] Detected change in script: {}", path_str);
                                            on_change(path_str);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        });

        self.handle = Some(handle);
    }

    pub fn stop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for ExtensionWatcher {
    fn drop(&mut self) {
        self.stop();
    }
}
