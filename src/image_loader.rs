#![allow(dead_code)]

#[derive(Debug, Clone)]
pub struct Image {
    pub url: String,
    pub width: u32,
    pub height: u32,
}

impl Image {
    pub fn new(url: &str, width: u32, height: u32) -> Self {
        Image { url: url.to_string(), width, height }
    }
}

pub struct ImageCache {
    inner: std::collections::HashMap<String, Image>,
}

impl ImageCache {
    pub fn new() -> Self {
        ImageCache { inner: std::collections::HashMap::new() }
    }
    
    pub fn len(&self) -> usize {
        self.inner.len()
    }
    
    pub fn add(&mut self, url: String, img: Image) {
        self.inner.insert(url, img);
    }
    
    pub fn get(&self, url: &str) -> Option<&Image> {
        self.inner.get(url)
    }
}
