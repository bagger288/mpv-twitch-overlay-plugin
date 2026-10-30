//! Emote and Badge in-memory caching and resizing

use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use tiny_skia::Pixmap;

pub struct EmoteCache {
    base_cache_dir: PathBuf,
    pixmap_cache: HashMap<(String, u32), Pixmap>, // key: (identifier, target_height)
}

impl EmoteCache {
    pub fn new(session_dir: &Path) -> Self {
        // Try emote_cache next to session_dir or in parent dirs
        let mut cache_dir = session_dir.join("emote_cache");
        if !cache_dir.exists() {
            if let Some(parent) = session_dir.parent() {
                let p_cache = parent.join("emote_cache");
                if p_cache.exists() {
                    cache_dir = p_cache;
                }
            }
        }

        Self {
            base_cache_dir: cache_dir,
            pixmap_cache: HashMap::new(),
        }
    }

    /// Loads image file, resizes to target_height, caches as Pixmap
    pub fn get_or_load(&mut self, rel_or_abs_path: &str, target_height: u32) -> Option<&Pixmap> {
        let key = (rel_or_abs_path.to_string(), target_height);
        if self.pixmap_cache.contains_key(&key) {
            return self.pixmap_cache.get(&key);
        }

        let full_path = if Path::new(rel_or_abs_path).is_absolute() {
            PathBuf::from(rel_or_abs_path)
        } else {
            self.base_cache_dir.join(rel_or_abs_path)
        };

        if !full_path.exists() {
            return None;
        }

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

        self.pixmap_cache.insert(key.clone(), pixmap);
        self.pixmap_cache.get(&key)
    }

    pub fn clear(&mut self) {
        self.pixmap_cache.clear();
    }
}
