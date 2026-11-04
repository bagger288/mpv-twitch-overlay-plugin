//! Shared, bounded image cache with asynchronous animation decoding.

use crate::animation::{self, Animation};
use parking_lot::Mutex;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant, SystemTime};
use tiny_skia::Pixmap;

const CACHE_BYTES: usize = 96 * 1024 * 1024;

type Key = (String, u32);

pub struct ImageSample {
    pub pixmap: Arc<Pixmap>,
    pub frame: usize,
    pub animated: bool,
    pub next_frame: Option<Duration>,
}

struct Cached {
    animation: Arc<Animation>,
    used: u64,
    stamp: Option<(u64, SystemTime)>,
}

struct Job { key: Key, path: PathBuf, height: u32 }
struct Finished { key: Key, animation: Option<Animation>, stamp: Option<(u64, SystemTime)> }

pub struct EmoteCache {
    session_dir: PathBuf,
    cache_candidates: Vec<PathBuf>,
    cached: Mutex<HashMap<Key, Cached>>,
    pending: Mutex<HashSet<Key>>,
    failed: Mutex<HashMap<Key, Instant>>,
    clock: Mutex<u64>,
    sender: mpsc::SyncSender<Job>,
    receiver: Mutex<mpsc::Receiver<Finished>>,
    started: Instant,
    dimension_cache: Mutex<HashMap<String, (u32, u32)>>,
    revision: Mutex<u64>,
}

impl EmoteCache {
    pub fn new(session_dir: &Path) -> Self { Self::with_cache_dir(session_dir, None) }

    pub fn with_cache_dir(session_dir: &Path, explicit: Option<&Path>) -> Self {
        let mut candidates = Vec::new();
        if let Some(path) = explicit { candidates.push(path.to_path_buf()); }
        candidates.push(session_dir.join("emote_cache"));
        for parent in session_dir.ancestors().skip(1).take(3) { candidates.push(parent.join("emote_cache")); }
        candidates.extend([PathBuf::from("emote_cache"), PathBuf::from("../emote_cache"), PathBuf::from("../../emote_cache")]);
        let (sender, jobs) = mpsc::sync_channel::<Job>(64);
        let (results, receiver) = mpsc::sync_channel(2);
        std::thread::spawn(move || {
            while let Ok(job) = jobs.recv() {
                let stamp = stamp(&job.path);
                let animation = match animation::decode(&job.path, job.height) {
                    Ok(animation) => Some(animation),
                    Err(reason) => { crate::mpv_api::log_plugin(&format!("Animation fallback: {}", reason)); None }
                };
                if results.send(Finished { key: job.key, animation, stamp }).is_err() { break; }
            }
        });
        Self { session_dir: session_dir.to_path_buf(), cache_candidates: candidates, cached: Mutex::new(HashMap::new()),
            pending: Mutex::new(HashSet::new()), failed: Mutex::new(HashMap::new()), clock: Mutex::new(0), sender,
            receiver: Mutex::new(receiver), started: Instant::now(), dimension_cache: Mutex::new(HashMap::new()), revision: Mutex::new(0) }
    }

    pub fn resolve_path(&self, value: &str) -> Option<PathBuf> {
        let path = Path::new(value);
        if path.components().any(|c| matches!(c, std::path::Component::ParentDir)) { return None; }
        if path.is_absolute() { return path.is_file().then(|| path.to_path_buf()); }
        for directory in &self.cache_candidates {
            let candidate = directory.join(path);
            if candidate.is_file() { return Some(candidate); }
        }
        let candidate = self.session_dir.join(path);
        candidate.is_file().then_some(candidate)
    }

    pub fn preferred_path(&self, preview: &str, animation_path: Option<&str>) -> String {
        if let Some(path) = animation_path.filter(|path| self.resolve_path(path).is_some()) { return path.to_string(); }
        if preview.starts_with("twitch/") {
            for extension in ["webp", "gif"] {
                let path = Path::new(preview).with_extension(extension).to_string_lossy().into_owned();
                if self.resolve_path(&path).is_some() { return path; }
            }
        }
        preview.to_string()
    }

    pub fn elapsed(&self) -> Duration { self.started.elapsed() }

    pub fn poll(&self) -> bool {
        let mut changed = false;
        while let Ok(result) = self.receiver.lock().try_recv() {
            self.pending.lock().remove(&result.key);
            if let Some(animation) = result.animation {
                self.insert(result.key, animation, result.stamp);
                changed = true;
            } else { self.failed.lock().insert(result.key, Instant::now()); }
        }
        changed
    }

    fn insert(&self, key: Key, animation: Animation, stamp: Option<(u64, SystemTime)>) {
        let mut cache = self.cached.lock();
        let mut used = self.clock.lock(); *used += 1;
        while cache.values().map(|entry| entry.animation.bytes).sum::<usize>() + animation.bytes > CACHE_BYTES {
            let oldest = cache.iter().min_by_key(|(_, entry)| entry.used).map(|(key, _)| key.clone());
            if let Some(oldest) = oldest { cache.remove(&oldest); } else { break; }
        }
        *self.revision.lock() += 1;
        cache.insert(key, Cached { animation: Arc::new(animation), used: *used, stamp });
    }

    pub fn request(&self, value: &str, height: u32) {
        let key = (value.to_string(), height);
        if self.cached.lock().contains_key(&key) || self.pending.lock().contains(&key) { return; }
        if self.failed.lock().get(&key).map(|at| at.elapsed() < Duration::from_secs(30)).unwrap_or(false) { return; }
        let Some(path) = self.resolve_path(value) else { return; };
        if !is_animation_path(&path) { return; }
        self.pending.lock().insert(key.clone());
        if self.sender.try_send(Job { key: key.clone(), path, height }).is_err() { self.pending.lock().remove(&key); }
    }

    pub fn sample(&self, value: &str, height: u32, elapsed: Duration) -> Option<ImageSample> {
        self.poll();
        let key = (value.to_string(), height);
        if let Some(entry) = self.cached.lock().get_mut(&key) {
            let mut clock = self.clock.lock(); *clock += 1; entry.used = *clock;
            let (frame, next_frame) = entry.animation.frame_at(elapsed);
            return Some(ImageSample { pixmap: entry.animation.frames[frame].clone(), frame, animated: entry.animation.frames.len() > 1, next_frame });
        }
        let path = self.resolve_path(value)?;
        if is_animation_path(&path) {
            self.request(value, height);
            let preview = path.with_extension("png");
            if !preview.is_file() { return None; }
            return self.sample(&preview.to_string_lossy(), height, elapsed);
        }
        let image = image::ImageReader::open(&path).ok()?.with_guessed_format().ok()?.decode().ok()?.to_rgba8();
        let pixmap = Arc::new(animation::resized_pixmap(&image, height)?);
        let result = ImageSample { pixmap: pixmap.clone(), frame: 0, animated: false, next_frame: None };
        self.insert(key, Animation { bytes: pixmap.data().len(), frames: vec![pixmap], ends_us: vec![1], loops: Some(1) }, stamp(&path));
        Some(result)
    }

    pub fn dimensions(&self, value: &str) -> Option<(u32, u32)> {
        if let Some(dimensions) = self.dimension_cache.lock().get(value).copied() { return Some(dimensions); }
        let path = self.resolve_path(value)?;
        let dimensions = image::ImageReader::open(path).ok()?.with_guessed_format().ok()?.into_dimensions().ok()?;
        self.dimension_cache.lock().insert(value.to_string(), dimensions);
        Some(dimensions)
    }

    pub fn revision(&self) -> u64 { *self.revision.lock() }

    pub fn get_width(&self, value: &str, height: u32) -> u32 {
        self.dimensions(value).map(|(w, h)| ((height as f64 * w as f64 / h.max(1) as f64).round() as u32).max(1)).unwrap_or(height)
    }

    pub fn get_or_load(&self, value: &str, height: u32) -> Option<Arc<Pixmap>> {
        self.sample(value, height, self.elapsed()).map(|sample| sample.pixmap)
    }

    pub fn pending_count(&self) -> usize { self.pending.lock().len() }

    pub fn invalidate_changed(&self) -> bool {
        let mut cache = self.cached.lock();
        let before = cache.len();
        cache.retain(|(path, _), entry| self.resolve_path(path).map(|path| stamp(&path)) == Some(entry.stamp));
        before != cache.len()
    }

    pub fn clear(&self) { self.cached.lock().clear(); self.failed.lock().clear(); }
}

fn stamp(path: &Path) -> Option<(u64, SystemTime)> {
    let metadata = std::fs::metadata(path).ok()?;
    Some((metadata.len(), metadata.modified().ok()?))
}

fn is_animation_path(path: &Path) -> bool {
    matches!(path.extension().and_then(|s| s.to_str()).map(|s| s.to_ascii_lowercase()).as_deref(), Some("webp" | "gif"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_cache_root_and_shared_static_images() {
        let root = std::env::temp_dir().join(format!("mpv-cache-root-{}", std::process::id()));
        let session = root.join("session"); let custom = root.join("custom");
        std::fs::create_dir_all(session.join("emote_cache")).unwrap();
        std::fs::create_dir_all(custom.join("7tv")).unwrap();
        let file = custom.join("7tv/test.png");
        Pixmap::new(56, 56).unwrap().save_png(&file).unwrap();
        let cache = EmoteCache::with_cache_dir(&session, Some(&custom));
        assert_eq!(cache.resolve_path("7tv/test.png"), Some(file.clone()));
        let first = cache.get_or_load("7tv/test.png", 28).unwrap();
        let second = cache.get_or_load("7tv/test.png", 28).unwrap();
        assert!(Arc::ptr_eq(&first, &second));
        assert!(cache.resolve_path("../test.png").is_none());
        std::fs::remove_dir_all(root).unwrap();
    }

    pub(crate) fn wait_decoded(cache: &EmoteCache, value: &str, height: u32) -> ImageSample {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(sample) = cache.sample(value, height, Duration::ZERO).filter(|sample| sample.animated) { return sample; }
            assert!(Instant::now() < deadline, "animation was not decoded in time");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn animated_preview_then_frames_advance_and_are_shared() {
        let root = std::env::temp_dir().join(format!("mpv-cache-anim-{}", std::process::id()));
        let cache_dir = root.join("emote_cache");
        let gif = cache_dir.join("7tv/anim.gif");
        crate::animation::write_test_gif(&gif, &[40, 60, 100], image::codecs::gif::Repeat::Infinite);
        Pixmap::new(8, 4).unwrap().save_png(cache_dir.join("7tv/anim.png")).unwrap();
        let cache = EmoteCache::with_cache_dir(&root, Some(&cache_dir));
        assert_eq!(cache.preferred_path("7tv/anim.png", Some("7tv/anim.gif")), "7tv/anim.gif");
        assert_eq!(cache.preferred_path("7tv/anim.png", Some("7tv/missing.gif")), "7tv/anim.png");

        // The first sample is the static preview while the worker decodes.
        let preview = cache.sample("7tv/anim.gif", 28, Duration::ZERO).unwrap();
        assert!(!preview.animated);
        let revision = cache.revision();
        let first = wait_decoded(&cache, "7tv/anim.gif", 28);
        assert!(cache.revision() > revision);
        assert_eq!((first.frame, first.next_frame), (0, Some(Duration::from_millis(40))));
        let second = cache.sample("7tv/anim.gif", 28, Duration::from_millis(50)).unwrap();
        assert_eq!((second.frame, second.next_frame), (1, Some(Duration::from_millis(50))));
        assert_eq!(cache.sample("7tv/anim.gif", 28, Duration::from_millis(210)).unwrap().frame, 0);
        assert_ne!(first.pixmap.data(), second.pixmap.data());
        // Duplicate emotes share decoded frames instead of copying them.
        let again = cache.sample("7tv/anim.gif", 28, Duration::from_millis(10)).unwrap();
        assert!(Arc::ptr_eq(&first.pixmap, &again.pixmap));

        // Replacing the file on disk evicts the decoded animation.
        assert!(!cache.invalidate_changed());
        std::thread::sleep(Duration::from_millis(20));
        crate::animation::write_test_gif(&gif, &[40, 60, 100, 100], image::codecs::gif::Repeat::Infinite);
        assert!(cache.invalidate_changed());
        let replaced = wait_decoded(&cache, "7tv/anim.gif", 28);
        assert_eq!(replaced.frame, 0);
        assert_eq!(cache.sample("7tv/anim.gif", 28, Duration::from_millis(250)).unwrap().frame, 3);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn broken_animation_keeps_png_preview() {
        let root = std::env::temp_dir().join(format!("mpv-cache-broken-{}", std::process::id()));
        let cache_dir = root.join("emote_cache");
        std::fs::create_dir_all(cache_dir.join("7tv")).unwrap();
        std::fs::write(cache_dir.join("7tv/bad.webp"), b"not a webp").unwrap();
        Pixmap::new(8, 4).unwrap().save_png(cache_dir.join("7tv/bad.png")).unwrap();
        let cache = EmoteCache::with_cache_dir(&root, Some(&cache_dir));
        cache.request("7tv/bad.webp", 28);
        let deadline = Instant::now() + Duration::from_secs(10);
        while cache.pending_count() > 0 || !cache.failed.lock().contains_key(&("7tv/bad.webp".to_string(), 28)) {
            cache.poll();
            assert!(Instant::now() < deadline, "decode failure was not reported");
            std::thread::sleep(Duration::from_millis(5));
        }
        let sample = cache.sample("7tv/bad.webp", 28, Duration::ZERO).unwrap();
        assert!(!sample.animated);
        assert_eq!(sample.pixmap.height(), 28);
        assert_eq!(cache.pending_count(), 0, "failed decodes are not retried immediately");
        std::fs::remove_dir_all(root).unwrap();
    }
}
