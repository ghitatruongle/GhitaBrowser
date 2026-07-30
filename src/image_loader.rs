// src/image_loader.rs - Image loading, caching, and decoding (v0.1.2)
#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::Arc;
use log::warn;

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
                            self.current_size = self.current_size.saturating_sub(old.rgba_pixels.len());
                        }
                    }
                }
                
                let data = Arc::new(data);
                self.current_size += size;
                self.decoded.insert(url.to_string(), data.clone());
                Some(data)
            }
            Err(e) => {
                warn!("Failed to load image {}: {}", url, e);
                None
            }
        }
    }
    
    /// Fetch image bytes from URL and decode
    fn fetch_and_decode(&self, url: &str) -> Result<ImageData, Box<dyn std::error::Error>> {
        // For now, try to use the image crate with files
        // In production, would fetch via HTTP
        let path = if url.starts_with("http://") || url.starts_with("https://") {
            // Remote image - would need real fetching
            return Err("Remote image loading not yet implemented".into());
        } else {
            url.to_string()
        };
        
        let img = image::open(&path)?;
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
        let img = Image::new("https://example.com/photo.jpg", 800, 600)
            .with_alt("A beautiful landscape");
        assert_eq!(img.alt_text, "A beautiful landscape");
    }

    #[test]
    fn test_format_from_extension() {
        assert_eq!(ImageFormat::from_extension("image.png"), ImageFormat::Png);
        assert_eq!(ImageFormat::from_extension("photo.jpg"), ImageFormat::Jpeg);
        assert_eq!(ImageFormat::from_extension("animation.gif"), ImageFormat::Gif);
    }
}
