//! Bounded animation decoding and original frame timing.

use image::{AnimationDecoder, ImageDecoder};
use std::io::BufReader;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tiny_skia::Pixmap;

pub const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;
pub const MAX_FRAMES: usize = 600;
pub const MAX_ASSET_BYTES: usize = 32 * 1024 * 1024;
pub const MAX_DECODED_PIXELS: u64 = 64 * 1024 * 1024;

pub struct Animation {
    pub frames: Vec<Arc<Pixmap>>,
    pub ends_us: Vec<u64>,
    pub loops: Option<u32>,
    pub bytes: usize,
}

impl Animation {
    pub fn frame_at(&self, elapsed: Duration) -> (usize, Option<Duration>) {
        let total = *self.ends_us.last().unwrap_or(&1);
        let elapsed = elapsed.as_micros().min(u64::MAX as u128) as u64;
        if self.frames.len() <= 1 || self.loops.map(|loops| elapsed >= total.saturating_mul(loops as u64)).unwrap_or(false) {
            return (self.frames.len().saturating_sub(1), None);
        }
        let local = elapsed % total;
        let index = self.ends_us.partition_point(|end| *end <= local).min(self.frames.len() - 1);
        (index, Some(Duration::from_micros(self.ends_us[index] - local)))
    }
}

pub fn resized_pixmap(image: &image::RgbaImage, height: u32) -> Option<Pixmap> {
    let (width, source_height) = image.dimensions();
    if width == 0 || source_height == 0 || height == 0 || height > 512 { return None; }
    let target_width = ((height as f64 * width as f64 / source_height as f64).round() as u32).max(1);
    if target_width > 4096 { return None; }
    let image = image::imageops::resize(image, target_width, height, image::imageops::FilterType::Triangle);
    let mut pixmap = Pixmap::new(target_width, height)?;
    for (src, dst) in image.as_raw().chunks_exact(4).zip(pixmap.data_mut().chunks_exact_mut(4)) {
        let alpha = src[3] as u32;
        for channel in 0..3 { dst[channel] = ((src[channel] as u32 * alpha + 127) / 255) as u8; }
        dst[3] = src[3];
    }
    Some(pixmap)
}

pub fn decode(path: &Path, height: u32) -> Result<Animation, String> {
    if std::fs::metadata(path).map_err(|_| "Image file unavailable")?.len() > MAX_FILE_BYTES { return Err("Image exceeds file size limit".into()); }
    let file = std::fs::File::open(path).map_err(|_| "Image file unavailable")?;
    let reader = BufReader::new(file);
    let format = path.extension().and_then(|s| s.to_str()).unwrap_or("").to_ascii_lowercase();
    match format.as_str() {
        "webp" => {
            let decoder = image::codecs::webp::WebPDecoder::new(reader).map_err(|_| "WebP decode failed")?;
            validate_dimensions(decoder.dimensions())?;
            let loops = loop_count(decoder.loop_count());
            frames(decoder.into_frames(), height, loops)
        }
        "gif" => {
            let decoder = image::codecs::gif::GifDecoder::new(reader).map_err(|_| "GIF decode failed")?;
            validate_dimensions(decoder.dimensions())?;
            // NETSCAPE2.0 stores extra repetitions: browsers play a GIF N+1 times.
            let loops = loop_count(decoder.loop_count()).map(|count| count.saturating_add(1));
            frames(decoder.into_frames(), height, loops)
        }
        _ => Err("Unsupported animation format".into()),
    }
}

fn validate_dimensions((width, height): (u32, u32)) -> Result<(), String> {
    if width == 0 || height == 0 || width > 2048 || height > 2048 || width as u64 * height as u64 > 4 * 1024 * 1024 {
        Err("Animation exceeds canvas limits".into())
    } else { Ok(()) }
}

fn loop_count(count: image::metadata::LoopCount) -> Option<u32> {
    match count {
        image::metadata::LoopCount::Infinite => None,
        image::metadata::LoopCount::Finite(count) => Some(count.get()),
    }
}

fn frames(frames: image::Frames<'_>, height: u32, loops: Option<u32>) -> Result<Animation, String> {
    let mut result = Animation { frames: Vec::new(), ends_us: Vec::new(), loops, bytes: 0 };
    let mut pixels = 0u64;
    let mut time = 0u64;
    for frame in frames {
        if result.frames.len() >= MAX_FRAMES { return Err("Animation exceeds frame limit".into()); }
        let frame = frame.map_err(|_| "Animation frame decode failed")?;
        pixels += frame.buffer().width() as u64 * frame.buffer().height() as u64;
        if pixels > MAX_DECODED_PIXELS { return Err("Animation exceeds decoded pixel limit".into()); }
        let (numerator, denominator) = frame.delay().numer_denom_ms();
        let duration = browser_frame_us(numerator as u64 * 1000 / denominator.max(1) as u64);
        time = time.checked_add(duration).ok_or("Animation duration overflow")?;
        let pixmap = resized_pixmap(frame.buffer(), height).ok_or("Animation resize failed")?;
        result.bytes += pixmap.data().len();
        if result.bytes > MAX_ASSET_BYTES { return Err("Animation exceeds memory limit".into()); }
        result.frames.push(Arc::new(pixmap));
        result.ends_us.push(time);
    }
    if result.frames.is_empty() { return Err("Animation has no frames".into()); }
    Ok(result)
}

/// Chromium and Firefox show GIF/WebP frames of <=10 ms for 100 ms.
fn browser_frame_us(delay_us: u64) -> u64 {
    if delay_us <= 10_000 { 100_000 } else { delay_us }
}

/// Writes a GIF whose frames have distinct solid colors (test fixtures only).
#[cfg(test)]
pub fn write_test_gif(path: &Path, delays_ms: &[u32], repeat: image::codecs::gif::Repeat) {
    use image::codecs::gif::GifEncoder;
    use image::{Delay, Frame, Rgba, RgbaImage};
    const COLORS: [[u8; 4]; 4] = [[255, 0, 0, 255], [0, 255, 0, 255], [0, 0, 255, 255], [255, 255, 255, 255]];
    if let Some(parent) = path.parent() { std::fs::create_dir_all(parent).unwrap(); }
    let mut encoder = GifEncoder::new(std::fs::File::create(path).unwrap());
    encoder.set_repeat(repeat).unwrap();
    for (index, delay) in delays_ms.iter().enumerate() {
        let image = RgbaImage::from_pixel(8, 4, Rgba(COLORS[index % COLORS.len()]));
        encoder.encode_frame(Frame::from_parts(image, 0, 0, Delay::from_numer_denom_ms(*delay, 1))).unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::codecs::gif::Repeat;

    fn fixture(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("mpv-animation-{}-{}", std::process::id(), name))
    }

    #[test]
    fn timing_keeps_variable_delays_and_skips_late_frames() {
        let frame = Arc::new(Pixmap::new(1, 1).unwrap());
        let animation = Animation { frames: vec![frame.clone(), frame.clone(), frame], ends_us: vec![16000, 33000, 83000], loops: None, bytes: 12 };
        assert_eq!(animation.frame_at(Duration::from_micros(15000)).0, 0);
        assert_eq!(animation.frame_at(Duration::from_micros(16000)).0, 1);
        assert_eq!(animation.frame_at(Duration::from_micros(70000)).0, 2);
        assert_eq!(animation.frame_at(Duration::from_micros(90000)).0, 0);
        assert_eq!(animation.frame_at(Duration::from_micros(16000)).1, Some(Duration::from_micros(17000)));
    }

    #[test]
    fn finite_loops_stop_on_last_frame() {
        let frame = Arc::new(Pixmap::new(1, 1).unwrap());
        let animation = Animation { frames: vec![frame.clone(), frame], ends_us: vec![10000, 20000], loops: Some(2), bytes: 8 };
        assert_eq!(animation.frame_at(Duration::from_millis(40)), (1, None));
    }

    #[test]
    fn gif_decode_uses_browser_delays_and_scales_frames() {
        let path = fixture("delays.gif");
        write_test_gif(&path, &[0, 30, 10, 20], Repeat::Infinite);
        let animation = decode(&path, 28).unwrap();
        std::fs::remove_file(&path).ok();
        assert_eq!(animation.ends_us, vec![100_000, 130_000, 230_000, 250_000]);
        assert_eq!(animation.loops, None);
        assert_eq!(animation.frames.len(), 4);
        assert_eq!((animation.frames[0].width(), animation.frames[0].height()), (56, 28));
        // Distinct premultiplied colors survive the single resize.
        assert_eq!(&animation.frames[0].data()[..4], &[255, 0, 0, 255]);
        assert_eq!(&animation.frames[1].data()[..4], &[0, 255, 0, 255]);
    }

    #[test]
    fn gif_repeat_count_matches_browsers() {
        let path = fixture("repeat.gif");
        write_test_gif(&path, &[50, 50], Repeat::Finite(2));
        let animation = decode(&path, 28).unwrap();
        std::fs::remove_file(&path).ok();
        assert_eq!(animation.loops, Some(3));
        assert_eq!(animation.frame_at(Duration::from_millis(250)).0, 1);
        assert_eq!(animation.frame_at(Duration::from_millis(300)), (1, None));
    }

    #[test]
    fn frame_limit_rejects_animation() {
        let path = fixture("limit.gif");
        write_test_gif(&path, &vec![20; MAX_FRAMES + 1], Repeat::Infinite);
        let result = decode(&path, 28);
        std::fs::remove_file(&path).ok();
        assert_eq!(result.err().as_deref(), Some("Animation exceeds frame limit"));
    }
}
