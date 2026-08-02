// src/image_loader.rs - Image loading, caching, and decoding (v0.6.1)


use log::warn;
use std::collections::HashMap;
use std::io::Read;
use std::sync::Arc;

/// Decoded image data
#[derive(Debug, Clone)]
pub struct ImageData {
    pub url: String,
    pub width: u32,
    pub height: u32,
    pub rgba_pixels: Vec<u8>,
    pub format: ImageFormat,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ImageFormat {
    Png,
    Jpeg,
    Gif,
    Webp,
    Bmp,
    Unknown,
}

impl ImageFormat {
    pub fn from_mime(mime: &str) -> Self {
        match mime {
            "image/png" => ImageFormat::Png,
            "image/jpeg" | "image/jpg" => ImageFormat::Jpeg,
            "image/gif" => ImageFormat::Gif,
            "image/webp" => ImageFormat::Webp,
            "image/bmp" => ImageFormat::Bmp,
            _ => ImageFormat::Unknown,
        }
    }

    pub fn from_extension(path: &str) -> Self {
        let ext = path.rsplit('.').next().unwrap_or("").to_lowercase();
        match ext.as_str() {
            "png" => ImageFormat::Png,
            "jpg" | "jpeg" => ImageFormat::Jpeg,
            "gif" => ImageFormat::Gif,
            "webp" => ImageFormat::Webp,
            "bmp" => ImageFormat::Bmp,
            _ => ImageFormat::Unknown,
        }
    }
}

/// A simplified image representation for the text renderer
#[derive(Debug, Clone)]
pub struct Image {
    pub url: String,
    pub width: u32,
    pub height: u32,
    pub alt_text: String,
    pub loaded: bool,
}

impl Image {
    pub fn new(url: &str, width: u32, height: u32) -> Self {
        Image {
            url: url.to_string(),
            width,
            height,
            alt_text: String::new(),
            loaded: false,
        }
    }

    pub fn with_alt(mut self, alt: &str) -> Self {
        self.alt_text = alt.to_string();
        self
    }
}

/// Image cache with memory management
pub struct ImageCache {
    inner: HashMap<String, Image>,
    decoded: HashMap<String, Arc<ImageData>>,
    max_cache_size: usize,
    current_size: usize,
}

impl Default for ImageCache {
    fn default() -> Self {
        Self::new()
    }
}

impl ImageCache {
    pub fn new() -> Self {
        ImageCache {
            inner: HashMap::new(),
            decoded: HashMap::new(),
            max_cache_size: 100 * 1024 * 1024, // 100MB
            current_size: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.inner.len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    pub fn add(&mut self, url: String, img: Image) {
        self.inner.insert(url, img);
    }

    pub fn get(&self, url: &str) -> Option<&Image> {
        self.inner.get(url)
    }

    /// Load and decode an image from URL (fetches and decodes)
    pub fn load_image(&mut self, url: &str) -> Option<Arc<ImageData>> {
        // Check decoded cache first
        if let Some(data) = self.decoded.get(url) {
            return Some(data.clone());
        }

        // Try to fetch and decode
        match self.fetch_and_decode(url) {
            Ok(data) => {
                let size = data.rgba_pixels.len();

                // Evict if needed
                while self.current_size + size > self.max_cache_size && !self.decoded.is_empty() {
                    if let Some(oldest_key) = self.decoded.keys().next().cloned() {
                        if let Some(old) = self.decoded.remove(&oldest_key) {
                            self.current_size =
                                self.current_size.saturating_sub(old.rgba_pixels.len());
                        }
                    }
                }

                let data = Arc::new(data);
                self.current_size += size;
                self.decoded.insert(url.to_string(), data.clone());
                // Mark the Image metadata as loaded so paint can detect it
                if let Some(img) = self.inner.get_mut(url) {
                    img.loaded = true;
                    img.width = data.width;
                    img.height = data.height;
                } else {
                    self.inner.insert(
                        url.to_string(),
                        Image::new(url, data.width, data.height),
                    );
                    if let Some(img) = self.inner.get_mut(url) {
                        img.loaded = true;
                    }
                }
                Some(data)
            }
            Err(e) => {
                warn!("Failed to load image {}: {}", url, e);
                None
            }
        }
    }

    /// Fetch image bytes from URL and decode. Only http(s) and file:// URLs
    /// are accepted; anything else (e.g. an arbitrary filesystem path slipped
    /// in through a crafted `src`) is rejected instead of being read as a file.
    fn fetch_and_decode(&self, url: &str) -> Result<ImageData, Box<dyn std::error::Error>> {
        let bytes = if url.starts_with("http://") || url.starts_with("https://") {
            // Fetch remote image via HTTP/HTTPS
            let agent = ureq::AgentBuilder::new()
                .timeout_connect(std::time::Duration::from_secs(10))
                .timeout_read(std::time::Duration::from_secs(30))
                .redirects(5)
                .user_agent(&crate::network::browser_ua())
                .build();
            let response = agent.get(url).call()?;

            // Cap the raw payload so a huge image cannot exhaust memory
            // (decode + rgba expansion multiplies the bytes further).
            const MAX_IMAGE_BYTES: u64 = 50 * 1024 * 1024;
            let mut bytes = Vec::new();
            response
                .into_reader()
                .take(MAX_IMAGE_BYTES + 1)
                .read_to_end(&mut bytes)?;
            if bytes.len() as u64 > MAX_IMAGE_BYTES {
                return Err("Image exceeds the 50MB download limit".into());
            }
            bytes
        } else if let Some(path) = url.strip_prefix("file://") {
            // Local file via an explicit file:// URL. Bare paths and other
            // schemes are rejected so a hostile src cannot read local files.
            std::fs::read(path)?
        } else {
            return Err(format!("Unsupported image URL scheme: {}", url).into());
        };

        let img = image::load_from_memory(&bytes)?;
        let rgba = img.to_rgba8();
        let (width, height) = rgba.dimensions();
        let pixels = rgba.into_raw();

        let format = ImageFormat::from_extension(url);

        Ok(ImageData {
            url: url.to_string(),
            width,
            height,
            rgba_pixels: pixels,
            format,
        })
    }

    /// Clear all cached images
    pub fn clear(&mut self) {
        self.inner.clear();
        self.decoded.clear();
        self.current_size = 0;
    }

    /// Get cache memory usage
    pub fn memory_usage(&self) -> usize {
        self.current_size
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_image_creation() {
        let img = Image::new("https://example.com/img.png", 100, 200);
        assert_eq!(img.url, "https://example.com/img.png");
        assert_eq!(img.width, 100);
        assert_eq!(img.height, 200);
    }

    #[test]
    fn test_image_cache_basics() {
        let mut cache = ImageCache::new();
        let img = Image::new("https://example.com/test.png", 50, 50);
        cache.add("https://example.com/test.png".to_string(), img);
        assert_eq!(cache.len(), 1);

        let retrieved = cache.get("https://example.com/test.png");
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().width, 50);
    }

    #[test]
    fn test_image_with_alt() {
        let img =
            Image::new("https://example.com/photo.jpg", 800, 600).with_alt("A beautiful landscape");
        assert_eq!(img.alt_text, "A beautiful landscape");
    }

    #[test]
    fn test_format_from_extension() {
        assert_eq!(ImageFormat::from_extension("image.png"), ImageFormat::Png);
        assert_eq!(ImageFormat::from_extension("photo.jpg"), ImageFormat::Jpeg);
        assert_eq!(
            ImageFormat::from_extension("animation.gif"),
            ImageFormat::Gif
        );
    }

    #[test]
    fn test_bare_path_is_not_read_as_file() {
        // A non-URL value must be rejected as an unsupported scheme, never
        // fed to std::fs::read (which would let a crafted src read local files).
        let mut cache = ImageCache::new();
        assert!(cache.load_image("logo.png").is_none());
        assert!(cache
            .load_image("data:image/png;base64,iVBORw0KGgo=")
            .is_none());
    }

    #[test]
    fn test_local_file_url_requires_valid_image() {
        // file:// URLs are allowed, but non-image content still fails decode
        let mut cache = ImageCache::new();
        assert!(cache.load_image("file:///definitely/missing.png").is_none());
    }
}
