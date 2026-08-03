# GhitaBrowser 🦀

![GhitaBrowser Logo](logo.png)

A Chrome-style lightweight browser written 100% in safe Rust, built from scratch with no external reference code.

## Overview

GhitaBrowser is a complete web browser implementation written entirely in safe Rust (v1.1.0). The UI is modeled after Google Chrome — tab strip, omnibox, three-dot menu, bookmarks bar, history, downloads, settings — and pages are now painted with **real pixels on a graphics canvas**, while the whole engine (networking, HTML/CSS parsing, layout, painting, JS, storage) remains 100% homemade Rust.

### v1.1.0 New Features — the "Toàn Bộ Trang" release

**Error handling improvements**
- ✅ **Human-readable error messages** — network errors are now translated into friendly messages (e.g., "Page took too long" instead of technical connection errors)
- ✅ **Retry logic with exponential backoff** — failed requests are automatically retried up to 2 times for transient errors (timeouts, 5xx server errors)

**Reader Mode upgrade**
- ✅ **DOM-based content extraction** — Reader Mode now uses proper HTML parser instead of line-by-line regex stripping
- ✅ **Semantic content detection** — finds `<article>`, `<main>` tags and scores divs by class/id patterns to find the main content
- ✅ **Metadata extraction** — extracts author/byline, published date, site name from meta tags (og:title, article:author, etc.)
- ✅ **Cleaner output** — strips nav, header, footer, aside, script, style, and hidden elements from extracted content

**Fallback chain**
- ✅ **Canvas → Reader Mode → Text fallback** — when pixel rendering fails (empty display list), automatically falls back to Reader Mode extraction for readable content
- ✅ **Longer search snippets** — search result snippets now display up to 500 characters instead of 240

**JavaScript-only page handling (new!)**
- ✅ **SPA detection** — detects JavaScript-only pages (YouTube, Twitter, React/Angular/Vue apps) using known markers and heuristics (script ratio, visible text density)
- ✅ **YouTube video info page** — when accessing YouTube, extracts the video ID from URL and shows a clean info page with video ID, embed link, and alternatives (instead of empty skeleton)
- ✅ **Empty/sparse content notice** — pages with large HTML but little visible content get a friendly notice explaining the JS limitation, plus any extracted text
- ✅ **Useful fallback messages** — instead of showing blank pages, users get clear explanations of why the page can't be rendered and what alternatives they have

### v0.6.1 New Features — the "Lõi Vặt & Hiệu Năng" release

**Bug fixes**
- ✅ **No more silent cookie domain fallback** — `network.rs` now uses the originating domain explicitly when a redirect URL can't be parsed, instead of silently falling back
- ✅ **Cache TTL parsing is stricter** — invalid `max-age` values now log a warning and fall back to 300s instead of silently accepting garbage
- ✅ **Real layout node counts** — `layout_nodes` in DevTools/render stats now shows the actual number of layout tree nodes instead of hardcoded `0`

**Performance**
- ✅ **Dead code directives removed** — all 14 `#![allow(dead_code)]` directives stripped; compiler now flags genuinely unused code
- ✅ **More accurate link underline width** — canvas underline now uses CJK-aware text width estimation instead of a rough character-count heuristic

**Image support**
- ✅ **`<img>` tags in the display list** — `Image` variant added to `DisplayItem`; canvas draws a placeholder box with alt text; decoded images show a green background when loaded

**Under the hood**
- ✅ **Image cache wired into Browser** — `ImageCache` is now a field on `Browser`, threaded through `build_display_list_with_cache`

### v0.5.0 New Features — the "Real Pixels" release

**Pixel graphics renderer (new!)**
- ✅ **Display-list painter** (`src/paint.rs`) — the layout tree is compiled into paint commands: background rects, borders, styled text runs, link regions
- ✅ **GPU/CPU canvas painting** — pages are drawn with real pixels via Iced Canvas (wgpu/tiny-skia), no more ASCII output
- ✅ **Clickable links** — link regions are hit-tested on click; relative URLs resolve against the page URL; cursor becomes a pointer over links
- ✅ **CSS colors** — named colors, `#rgb`/`#rrggbb`/`#rrggbbaa`, `rgb()`/`rgba()` for text, backgrounds and borders
- ✅ **UA stylesheet** — Chrome-like default sizes for `h1`–`h6`, bold/italic/monospace inheritance, list bullets, `<hr>` dividers
- ✅ **White page sheet** — like Chrome, web content stays light even in dark UI theme; page is centered as a document sheet
- ✅ **Zoom-aware painting** — the canvas scales with the Chrome zoom steps (25%–500%)
- ✅ **Renderer setting** — switch between "Pixels (Chrome-like)" and legacy text mode in `ghita://settings`

### v0.3.0 Features — the "Chrome" release

**Chrome-style UI**
- ✅ **Chrome color palettes** — Pixel-matched dark (`#202124`/`#35363A`/`#8AB4F8`) and light (`#DEE1E6`/`#FFFFFF`/`#1A73E8`) themes
- ✅ **Chrome tab strip** — Rounded tabs attached to the toolbar, favicon glyphs, in-tab close buttons, `+` button, Chrome-style "activate right neighbor" on close
- ✅ **Omnibox** — Unified search/address pill with security chip (🔒/⚠), bookmark star (★) inside the box, real keyboard focus (`Ctrl+L`), and dropdown suggestions from history & bookmarks
- ✅ **Search from the address bar** — Non-URL input opens the in-app results page (`ghita://search`) using your chosen engine (Google, Bing or DuckDuckGo)
- ✅ **Three-dot menu (⋮)** — New tab, incognito, history, downloads, bookmarks, zoom controls, find, save page, settings, DevTools, about
- ✅ **Bookmarks bar** — Toggleable (`Ctrl+Shift+B`), one-click bookmark buttons
- ✅ **New Tab page** — Colored wordmark, centered search box, most-visited tiles from real browsing history
- ✅ **Loading strip** — Thin Chrome-style accent bar under the toolbar while fetching

**Chrome features (all persisted to disk)**
- ✅ **Bookmarks** — Star button / `Ctrl+D`, bookmark manager page (`ghita://bookmarks`)
- ✅ **Global browsing history** — `ghita://history` with search, per-entry delete, clear all (`Ctrl+H`)
- ✅ **Downloads manager** — "Save page as..." downloads to your Downloads folder; `ghita://downloads` lists files with size/date/status (`Ctrl+J`)
- ✅ **Settings page** — `ghita://settings`: theme, search engine, homepage, bookmarks bar, default zoom, clear browsing data
- ✅ **Incognito tabs** — `Ctrl+Shift+N`; visits are never recorded in history
- ✅ **Reopen closed tab** — `Ctrl+Shift+T` restores recently closed tabs
- ✅ **Find in page** — `Ctrl+F` bar with live match count
- ✅ **Zoom** — `Ctrl +` / `Ctrl -` / `Ctrl 0` with real Chrome zoom steps (25%–500%)
- ✅ **Internal pages** — `ghita://newtab`, `history`, `bookmarks`, `downloads`, `settings`, `search`, `about`, `incognito` (like `chrome://`)
- ✅ **Full Chrome keyboard shortcut set** — see table below

### Core Engine (Retained & Improved)

- ✅ **Real HTTP/HTTPS networking** via `ureq` + binary file downloads
- ✅ **HTML5 parser** with error recovery, script/style raw text, HTML entities, comments
- ✅ **Advanced CSS engine** with class/ID/tag selectors, specificity, shorthand properties, 20+ CSS properties
- ✅ **Layout engine** with text wrapping, auto-height, percentage widths, full box model
- ✅ **JavaScript engine** with variables (`let`), functions, `if`/`while` control flow, `console.log`
- ✅ **Persistent storage** via `serde`+`serde_json` — cookies, localStorage, bookmarks, history, downloads, settings
- ✅ **Resource cache** with TTL-based expiry and hit/miss tracking
- ✅ **104 tests** across all modules (90 unit + 14 integration)

### Core Components

| Module | Description | Status |
|--------|-------------|--------|
| **Network** | HTTP/HTTPS via ureq, ResourceCache with TTL, retry logic, binary downloads | ✅ v1.1.0 |
| **HTML Parser** | HTML5 tokenizer, error recovery, DOM tree | ✅ v1.1.0 |
| **CSS Parser** | Selectors (tag/class/id), specificity, 20+ properties | ✅ v1.1.0 |
| **Layout** | Box model, text wrapping, block/inline, auto-height, UA font sizes | ✅ v1.1.0 |
| **Paint** | Display-list painter: rects, borders, text runs, link hit-testing | ✅ v1.1.0 |
| **Renderer** | Real pixel canvas (wgpu/tiny-skia) + legacy text mode | ✅ v1.1.0 |
| **JavaScript** | Variables, functions, if/while, console API | ✅ v1.1.0 |
| **Storage** | Cookies, localStorage, bookmarks, history, downloads, settings | ✅ v1.1.0 |
| **UI (Iced)** | Chrome-style tab strip, omnibox, menu, bookmarks bar, internal pages, DevTools | ✅ v1.1.0 |
| **Tabs** | Chrome close behavior, incognito, reopen closed, tab cycling | ✅ v1.1.0 |
| **Reader Mode** | DOM-based content extraction, semantic detection, fallback chain | ✅ v1.1.0 |
| **Image Loader** | Image cache with memory management | ✅ v0.6.1 |
| **Performance** | Profiler with per-phase timing | ✅ v0.6.1 |
| **Search** | In-app results page via DuckDuckGo HTML endpoint | ✅ v0.6.1 |

## Keyboard Shortcuts (Chrome bindings)

| Shortcut | Action |
|----------|--------|
| `Ctrl + L` / `F6` | Focus & select the omnibox |
| `Ctrl + T` | New tab |
| `Ctrl + Shift + N` | New Incognito tab |
| `Ctrl + Shift + T` | Reopen closed tab |
| `Ctrl + W` | Close current tab |
| `Ctrl + Tab` / `Ctrl + Shift + Tab` | Next / previous tab |
| `Ctrl + 1..8`, `Ctrl + 9` | Jump to tab N / last tab |
| `Ctrl + R` / `F5` | Reload page |
| `Ctrl + D` | Bookmark this page |
| `Ctrl + Shift + B` | Toggle bookmarks bar |
| `Ctrl + Shift + O` | Bookmark manager |
| `Ctrl + H` | History |
| `Ctrl + J` | Downloads |
| `Ctrl + F` | Find in page |
| `Ctrl + =` / `Ctrl + -` / `Ctrl + 0` | Zoom in / out / reset |
| `Alt + ←` / `Alt + →` | Back / forward |
| `Alt + Home` | Go to homepage |
| `F12` / `Ctrl + Shift + I` | Developer tools |
| `Esc` | Close menu / suggestions / find bar / DevTools |

## Internal Pages

| URL | Page |
|-----|------|
| `ghita://newtab` | New Tab page with search & top sites |
| `ghita://history` | Browsing history |
| `ghita://bookmarks` | Bookmark manager |
| `ghita://downloads` | Downloads |
| `ghita://settings` | Settings |
| `ghita://search?q=...` | In-app search results |
| `ghita://about` | About GhitaBrowser |

## Building

```bash
# Clone the repository
git clone https://github.com/GhitaBrowser/ghitabrowser.git
cd ghitabrowser

# Build (release)
cargo build --release

# Run
cargo run --release

# Run tests
cargo test
```

## Dependencies

- **iced** 0.12 - GUI framework (window creation, widgets, canvas)
- **ureq** 2.9 - HTTP/HTTPS client
- **serde** + **serde_json** - Serialization
- **image** 0.24 - Image decoding
- **chrono** 0.4 - Date/time for cookies & history
- **url** 2.5 - URL parsing & query encoding
- **dirs** 5.0 - Downloads / AppData folders
- **log** + **env_logger** - Logging

## Development

See [CONTRIBUTING.md](CONTRIBUTING.md) for guidelines.

## License

MIT License - See LICENSE file for details.
