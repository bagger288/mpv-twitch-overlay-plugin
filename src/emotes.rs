//! Emote and Badge in-memory caching and resizing (Twitch, 7TV, BTTV, FFZ)

use parking_lot::Mutex;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tiny_skia::Pixmap;

pub struct EmoteCache {
    session_dir: PathBuf,
    base_cache_dir: PathBuf,
    pixmap_cache: Mutex<HashMap<(String, u32), Pixmap>>,
    dimension_cache: Mutex<HashMap<(String, u32), u32>>,
}

impl EmoteCache {
    pub fn new(session_dir: &Path) -> Self {
        // Search for emote_cache dir in session_dir and parent directories
        let mut candidates = Vec::new();
        candidates.push(session_dir.join("emote_cache"));

        if let Some(p1) = session_dir.parent() {
            candidates.push(p1.join("emote_cache"));
            if let Some(p2) = p1.parent() {
                candidates.push(p2.join("emote_cache"));
                if let Some(p3) = p2.parent() {
                    candidates.push(p3.join("emote_cache"));
                }
            }
        }

        candidates.push(PathBuf::from("emote_cache"));
        candidates.push(PathBuf::from("../emote_cache"));
        candidates.push(PathBuf::from("../../emote_cache"));

        let chosen = candidates
            .into_iter()
            .find(|p| p.is_dir() && p.exists())
            .unwrap_or_else(|| {
                // Default to project root: session_dir/../../emote_cache
                session_dir
                    .parent()
                    .and_then(|p| p.parent())
                    .map(|p| p.join("emote_cache"))
                    .unwrap_or_else(|| session_dir.join("emote_cache"))
            });

        Self {
            session_dir: session_dir.to_path_buf(),
            base_cache_dir: chosen,
            pixmap_cache: Mutex::new(HashMap::new()),
            dimension_cache: Mutex::new(HashMap::new()),
        }
    }

    /// Resolves relative or absolute path against base_cache_dir or session_dir with extension fallbacks
    pub fn resolve_path(&self, rel_or_abs_path: &str) -> Option<PathBuf> {
        let p = Path::new(rel_or_abs_path);
        if p.is_absolute() && p.exists() {
            return Some(p.to_path_buf());
        }

        // 1. Direct path in emote_cache
        let in_cache = self.base_cache_dir.join(rel_or_abs_path);
        if in_cache.exists() {
            return Some(in_cache);
        }

        // 2. Extension fallbacks (.png, .webp) in emote_cache
        let with_png = in_cache.with_extension("png");
        if with_png.exists() {
            return Some(with_png);
        }
        let with_webp = in_cache.with_extension("webp");
        if with_webp.exists() {
            return Some(with_webp);
        }

        // 3. Fallback inside session_dir
        let in_session = self.session_dir.join(rel_or_abs_path);
        if in_session.exists() {
            return Some(in_session);
        }

        None
    }

    /// Queries exact width for an emote image at given target_height (proportional aspect ratio)
    pub fn get_width(&self, rel_or_abs_path: &str, target_height: u32) -> u32 {
        let key = (rel_or_abs_path.to_string(), target_height);
        {
            let guard = self.dimension_cache.lock();
            if let Some(&w) = guard.get(&key) {
                return w;
            }
        }

        if let Some(px) = self.get_or_load(rel_or_abs_path, target_height) {
            let w = px.width();
            self.dimension_cache.lock().insert(key, w);
            return w;
        }

        // Default square fallback
        target_height
    }

    /// Loads image file, resizes to target_height with aspect ratio preservation, caches as Pixmap
    pub fn get_or_load(&self, rel_or_abs_path: &str, target_height: u32) -> Option<Pixmap> {
        let key = (rel_or_abs_path.to_string(), target_height);
        {
            let guard = self.pixmap_cache.lock();
            if let Some(px) = guard.get(&key) {
                return Some(px.clone());
            }
        }

        let full_path = self.resolve_path(rel_or_abs_path)?;

        let dynamic_img = image::ImageReader::open(&full_path)
            .ok()?
            .with_guessed_format()
            .ok()?
            .decode()
            .ok()?;

        let rgba_img = dynamic_img.to_rgba8();
        let (src_w, src_h) = rgba_img.dimensions();
        if src_w == 0 || src_h == 0 {
            return None;
        }

        let aspect = src_w as f32 / src_h as f32;
        let target_width = ((target_height as f32 * aspect).round() as u32).max(1);

        let resized = image::imageops::resize(
            &rgba_img,
            target_width,
            target_height,
            image::imageops::FilterType::Triangle,
        );

        let mut pixmap = Pixmap::new(target_width, target_height)?;
        let pixels = pixmap.data_mut();

        // Convert RGBA -> tiny-skia premultiplied RGBA
        for (i, p) in resized.pixels().enumerate() {
            let offset = i * 4;
            let r = p[0] as f32 / 255.0;
            let g = p[1] as f32 / 255.0;
            let b = p[2] as f32 / 255.0;
            let a = p[3] as f32 / 255.0;

            pixels[offset + 0] = (r * a * 255.0).round() as u8;
            pixels[offset + 1] = (g * a * 255.0).round() as u8;
            pixels[offset + 2] = (b * a * 255.0).round() as u8;
            pixels[offset + 3] = (a * 255.0).round() as u8;
        }

        self.pixmap_cache.lock().insert(key.clone(), pixmap.clone());
        self.dimension_cache.lock().insert(key, target_width);
        Some(pixmap)
    }

    pub fn clear(&self) {
        self.pixmap_cache.lock().clear();
        self.dimension_cache.lock().clear();
    }
}
