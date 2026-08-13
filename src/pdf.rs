//! Bounded, independently implemented PDF document reader.
//!
//! The first release focuses on safe structure inspection and readable text.
//! It intentionally rejects encryption and caps objects, pages, streams and
//! extracted text before allocating from untrusted values.

use std::collections::HashMap;
use std::io::Read;

const MAX_PDF_BYTES: usize = 50 * 1024 * 1024;
const MAX_OBJECTS: usize = 20_000;
const MAX_PAGES: usize = 2_000;
const MAX_STREAM_BYTES: u64 = 16 * 1024 * 1024;
const MAX_PAGE_TEXT_BYTES: usize = 2 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq)]
pub struct PdfPage {
    pub number: usize,
    pub width_points: f32,
    pub height_points: f32,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PdfDocument {
    pub version: String,
    pub title: String,
    pub object_count: usize,
    pub pages: Vec<PdfPage>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PdfError {
    TooLarge,
    InvalidHeader,
    Encrypted,
    TooManyObjects,
    TooManyPages,
    NoPages,
    InvalidStream(String),
}

impl std::fmt::Display for PdfError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooLarge => write!(formatter, "PDF exceeds the 50 MB document limit"),
            Self::InvalidHeader => write!(formatter, "File does not have a valid PDF header"),
            Self::Encrypted => write!(formatter, "Encrypted PDF files are not supported yet"),
            Self::TooManyObjects => write!(formatter, "PDF exceeds the object-count limit"),
            Self::TooManyPages => write!(formatter, "PDF exceeds the page-count limit"),
            Self::NoPages => write!(formatter, "PDF does not contain a readable page tree"),
            Self::InvalidStream(error) => write!(formatter, "Invalid PDF stream: {error}"),
        }
    }
}

impl std::error::Error for PdfError {}

#[derive(Debug)]
struct PdfObject<'a> {
    number: u32,
    body: &'a [u8],
}

pub fn parse(bytes: &[u8], fallback_title: &str) -> Result<PdfDocument, PdfError> {
    if bytes.len() > MAX_PDF_BYTES {
        return Err(PdfError::TooLarge);
    }
    if !bytes.starts_with(b"%PDF-") {
        return Err(PdfError::InvalidHeader);
    }
    if contains(bytes, b"/Encrypt") {
        return Err(PdfError::Encrypted);
    }

    let version_end = bytes
        .iter()
        .position(|byte| matches!(byte, b'\r' | b'\n'))
        .unwrap_or(bytes.len().min(16));
    let version = String::from_utf8_lossy(&bytes[5..version_end])
        .trim()
        .to_string();
    let objects = scan_objects(bytes)?;
    let object_map: HashMap<u32, &[u8]> = objects
        .iter()
        .map(|object| (object.number, object.body))
        .collect();

    let title = extract_info_string(bytes, b"/Title")
        .filter(|title| !title.trim().is_empty())
        .unwrap_or_else(|| fallback_title.to_string());

    let mut pages = Vec::new();
    for object in &objects {
        if !is_page_object(object.body) {
            continue;
        }
        if pages.len() >= MAX_PAGES {
            return Err(PdfError::TooManyPages);
        }
        let (width_points, height_points) = media_box(object.body).unwrap_or((612.0, 792.0));
        let mut text = String::new();
        let references = content_references(object.body);
        if references.is_empty() && contains(object.body, b"stream") {
            append_stream_text(object.body, &mut text)?;
        } else {
            for reference in references {
                if let Some(body) = object_map.get(&reference) {
                    append_stream_text(body, &mut text)?;
                }
                if text.len() >= MAX_PAGE_TEXT_BYTES {
                    break;
                }
            }
        }
        text.truncate(MAX_PAGE_TEXT_BYTES);
        pages.push(PdfPage {
            number: pages.len() + 1,
            width_points,
            height_points,
            text: normalize_text(&text),
        });
    }
    if pages.is_empty() {
        return Err(PdfError::NoPages);
    }

    Ok(PdfDocument {
        version,
        title,
        object_count: objects.len(),
        pages,
    })
}

pub fn render_to_html(bytes: &[u8], fallback_title: &str) -> Result<String, PdfError> {
    let document = parse(bytes, fallback_title)?;
    let mut html = String::with_capacity(
        document
            .pages
            .iter()
            .map(|page| page.text.len())
            .sum::<usize>()
            + 2048,
    );
    html.push_str("<!doctype html><html><head><meta charset=\"utf-8\"><title>");
    html.push_str(&html_escape(&document.title));
    html.push_str(
        "</title><style>body{background:#e8eaed;color:#202124;font-family:sans-serif;padding:24px}\
         .pdf-meta{background:#202124;color:white;padding:12px;margin-bottom:18px}\
         .pdf-page{background:white;color:#111;padding:28px;margin:0 auto 20px auto;width:90%;\
         border:1px solid #c7c7c7}pre{white-space:pre-wrap;font-family:sans-serif;line-height:1.45}\
         .empty{color:#666;font-style:italic}</style></head><body>",
    );
    html.push_str("<div class=\"pdf-meta\"><h1>");
    html.push_str(&html_escape(&document.title));
    html.push_str("</h1><p>PDF ");
    html.push_str(&html_escape(&document.version));
    html.push_str(" · ");
    html.push_str(&document.pages.len().to_string());
    html.push_str(" pages · ");
    html.push_str(&document.object_count.to_string());
    html.push_str(" objects</p></div>");
    for page in &document.pages {
        html.push_str("<section class=\"pdf-page\"><h2>Page ");
        html.push_str(&page.number.to_string());
        html.push_str("</h2><p>");
        html.push_str(&format!(
            "{:.0} × {:.0} pt",
            page.width_points, page.height_points
        ));
        html.push_str("</p>");
        if page.text.is_empty() {
            html.push_str(
                "<p class=\"empty\">This page has no extractable text. Image-only and complex font rendering will be added in a later compatibility milestone.</p>",
            );
        } else {
            html.push_str("<pre>");
            html.push_str(&html_escape(&page.text));
            html.push_str("</pre>");
        }
        html.push_str("</section>");
    }
    html.push_str("</body></html>");
    Ok(html)
}

fn scan_objects(bytes: &[u8]) -> Result<Vec<PdfObject<'_>>, PdfError> {
    let mut objects = Vec::new();
    let mut cursor = 0;
    while let Some(relative_obj) = find_subslice(&bytes[cursor..], b" obj") {
        let obj_marker = cursor + relative_obj;
        let header_start = bytes[..obj_marker]
            .iter()
            .rposition(|byte| matches!(byte, b'\r' | b'\n'))
            .map(|position| position + 1)
            .unwrap_or(0);
        let header = String::from_utf8_lossy(&bytes[header_start..obj_marker]);
        let mut parts = header.split_ascii_whitespace().rev();
        let generation = parts.next().and_then(|part| part.parse::<u32>().ok());
        let number = parts.next().and_then(|part| part.parse::<u32>().ok());
        let body_start = obj_marker + 4;
        let Some(relative_end) = find_subslice(&bytes[body_start..], b"endobj") else {
            break;
        };
        let body_end = body_start + relative_end;
        if generation.is_some() {
            if let Some(number) = number {
                objects.push(PdfObject {
                    number,
                    body: &bytes[body_start..body_end],
                });
                if objects.len() > MAX_OBJECTS {
                    return Err(PdfError::TooManyObjects);
                }
            }
        }
        cursor = body_end + 6;
    }
    Ok(objects)
}

fn is_page_object(body: &[u8]) -> bool {
    let Some(position) = find_subslice(body, b"/Type") else {
        return false;
    };
    let tail = &body[position + 5..];
    let Some(page_position) = find_subslice(tail, b"/Page") else {
        return false;
    };
    !tail
        .get(page_position + 5)
        .is_some_and(|next| next.is_ascii_alphabetic())
}

fn media_box(body: &[u8]) -> Option<(f32, f32)> {
    let position = find_subslice(body, b"/MediaBox")?;
    let tail = String::from_utf8_lossy(&body[position + 9..body.len().min(position + 160)]);
    let start = tail.find('[')?;
    let end = tail[start + 1..].find(']')? + start + 1;
    let values: Vec<f32> = tail[start + 1..end]
        .split_ascii_whitespace()
        .filter_map(|part| part.parse().ok())
        .take(4)
        .collect();
    (values.len() == 4).then(|| {
        (
            (values[2] - values[0]).abs().max(1.0),
            (values[3] - values[1]).abs().max(1.0),
        )
    })
}

fn content_references(body: &[u8]) -> Vec<u32> {
    let Some(position) = find_subslice(body, b"/Contents") else {
        return Vec::new();
    };
    let tail = String::from_utf8_lossy(&body[position + 9..body.len().min(position + 1024)]);
    let tokens: Vec<&str> = tail.split_ascii_whitespace().collect();
    let mut references = Vec::new();
    for window in tokens.windows(3) {
        if window[2].trim_matches(|character| matches!(character, '[' | ']')) == "R" {
            if let Ok(number) = window[0].trim_matches('[').parse::<u32>() {
                references.push(number);
            }
        }
        if references.len() >= 256 {
            break;
        }
    }
    references
}

fn append_stream_text(body: &[u8], output: &mut String) -> Result<(), PdfError> {
    let Some(marker) = find_subslice(body, b"stream") else {
        return Ok(());
    };
    let mut start = marker + 6;
    if body.get(start) == Some(&b'\r') {
        start += 1;
    }
    if body.get(start) == Some(&b'\n') {
        start += 1;
    }
    let Some(relative_end) = find_subslice(&body[start..], b"endstream") else {
        return Err(PdfError::InvalidStream("missing endstream".to_string()));
    };
    let raw = &body[start..start + relative_end];
    let decoded = if contains(&body[..marker], b"/FlateDecode") {
        let mut decoder = flate2::read::ZlibDecoder::new(raw);
        let mut decoded = Vec::new();
        decoder
            .by_ref()
            .take(MAX_STREAM_BYTES + 1)
            .read_to_end(&mut decoded)
            .map_err(|error| PdfError::InvalidStream(error.to_string()))?;
        if decoded.len() as u64 > MAX_STREAM_BYTES {
            return Err(PdfError::InvalidStream(
                "decoded stream exceeds limit".to_string(),
            ));
        }
        decoded
    } else {
        raw.iter()
            .copied()
            .take(MAX_STREAM_BYTES as usize)
            .collect()
    };
    if contains(&decoded, b"BT") && contains(&decoded, b"ET") {
        extract_literal_strings(&decoded, output);
    }
    Ok(())
}

fn extract_literal_strings(stream: &[u8], output: &mut String) {
    let mut index = 0;
    while index < stream.len() && output.len() < MAX_PAGE_TEXT_BYTES {
        if stream[index] != b'(' {
            index += 1;
            continue;
        }
        index += 1;
        let mut depth = 1_u16;
        let mut decoded = Vec::new();
        while index < stream.len() && depth > 0 && decoded.len() < MAX_PAGE_TEXT_BYTES {
            match stream[index] {
                b'\\' => {
                    index += 1;
                    if index >= stream.len() {
                        break;
                    }
                    match stream[index] {
                        b'n' => decoded.push(b'\n'),
                        b'r' => decoded.push(b'\r'),
                        b't' => decoded.push(b'\t'),
                        b'b' => decoded.push(8),
                        b'f' => decoded.push(12),
                        b'\r' => {
                            if stream.get(index + 1) == Some(&b'\n') {
                                index += 1;
                            }
                        }
                        b'\n' => {}
                        digit @ b'0'..=b'7' => {
                            let mut value = (digit - b'0') as u16;
                            for _ in 0..2 {
                                if let Some(next @ b'0'..=b'7') = stream.get(index + 1).copied() {
                                    index += 1;
                                    value = value * 8 + (next - b'0') as u16;
                                }
                            }
                            decoded.push(value.min(255) as u8);
                        }
                        other => decoded.push(other),
                    }
                }
                b'(' => {
                    depth = depth.saturating_add(1);
                    decoded.push(b'(');
                }
                b')' => {
                    depth -= 1;
                    if depth > 0 {
                        decoded.push(b')');
                    }
                }
                byte => decoded.push(byte),
            }
            index += 1;
        }
        let text = decode_pdf_text(&decoded);
        if !text.trim().is_empty() {
            if !output.is_empty() {
                output.push(' ');
            }
            output.push_str(text.trim());
        }
    }
}

fn decode_pdf_text(bytes: &[u8]) -> String {
    if bytes.starts_with(&[0xFE, 0xFF]) {
        let words: Vec<u16> = bytes[2..]
            .chunks_exact(2)
            .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
            .collect();
        return String::from_utf16_lossy(&words);
    }
    String::from_utf8_lossy(bytes).into_owned()
}

fn extract_info_string(bytes: &[u8], key: &[u8]) -> Option<String> {
    let position = find_subslice(bytes, key)? + key.len();
    let tail = &bytes[position..bytes.len().min(position + 4096)];
    let start = tail.iter().position(|byte| *byte == b'(')?;
    let mut output = String::new();
    extract_literal_strings(&tail[start..], &mut output);
    (!output.is_empty()).then_some(output)
}

fn normalize_text(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    find_subslice(haystack, needle).is_some()
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn simple_pdf(stream: &[u8], filter: &str) -> Vec<u8> {
        let mut pdf = format!(
            "%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj\n\
             2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj\n\
             3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 600 800] /Contents 4 0 R >> endobj\n\
             4 0 obj << /Length {} {} >>\nstream\n",
            stream.len(),
            filter
        )
        .into_bytes();
        pdf.extend_from_slice(stream);
        pdf.extend_from_slice(b"\nendstream\nendobj\n%%EOF");
        pdf
    }

    #[test]
    fn parses_page_size_and_text() {
        let pdf = simple_pdf(b"BT /F1 12 Tf (Hello PDF) Tj ET", "");
        let document = parse(&pdf, "sample.pdf").unwrap();
        assert_eq!(document.version, "1.7");
        assert_eq!(document.pages.len(), 1);
        assert_eq!(document.pages[0].width_points, 600.0);
        assert_eq!(document.pages[0].text, "Hello PDF");
        assert!(render_to_html(&pdf, "sample.pdf")
            .unwrap()
            .contains("Hello PDF"));
    }

    #[test]
    fn decodes_flate_stream_and_rejects_encryption() {
        use std::io::Write;
        let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
        encoder.write_all(b"BT (Compressed text) Tj ET").unwrap();
        let compressed = encoder.finish().unwrap();
        let pdf = simple_pdf(&compressed, "/Filter /FlateDecode");
        assert_eq!(
            parse(&pdf, "compressed.pdf").unwrap().pages[0].text,
            "Compressed text"
        );

        let encrypted = b"%PDF-1.7\n1 0 obj << /Encrypt 2 0 R >> endobj";
        assert_eq!(parse(encrypted, "locked.pdf"), Err(PdfError::Encrypted));
    }
}
