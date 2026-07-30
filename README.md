# GhitaBrowser 🦀

![GhitaBrowser Logo](logo.png)

A lightweight Rust browser optimized faster than Chrome, built from scratch with no external reference code or libraries.

## Overview

GhitaBrowser is a complete web browser implementation written entirely in safe Rust (v0.0.1).

### v0.0.1 New Features

- ✅ **Real HTTP/HTTPS networking** via `ureq` (was stub)
- ✅ **HTML5 parser** with error recovery, script/style raw text, HTML entities, comments
- ✅ **Advanced CSS engine** with class/ID/tag selectors, specificity, shorthand properties, 20+ CSS properties
- ✅ **Layout engine** with text wrapping, auto-height, percentage widths, full box model
- ✅ **JavaScript engine** with variables (`let`), functions, `if`/`while` control flow, `console.log`
- ✅ **Persistent storage** via `serde`+`serde_json` for cookies & localStorage
- ✅ **Full UI integration** connecting GUI with real Browser engine, cache, storage
- ✅ **74 unit tests** across all modules
- ✅ **Resource cache** with TTL-based expiry and hit/miss tracking

### Core Components

| Module | Description | Status |
|--------|-------------|--------|
| **Network** | Real HTTP/HTTPS via ureq, ResourceCache with TTL | ✅ v0.0.1 |
| **HTML Parser** | HTML5 tokenizer, error recovery, DOM tree | ✅ v0.0.1 |
| **CSS Parser** | Selectors (tag/class/id), specificity, 20+ properties | ✅ v0.0.1 |
| **Layout** | Box model, text wrapping, block/inline, auto-height | ✅ v0.0.1 |
| **Renderer** | ASCII text rendering of layout tree | ✅ v0.0.1 |
| **JavaScript** | Variables, functions, if/while, console API | ✅ v0.0.1 |
| **Storage** | CookieStore + LocalStorage with serde persistence | ✅ v0.0.1 |
| **UI (Iced)** | Tab bar, navigation, URL bar, status bar, dev tools | ✅ v0.0.1 |
| **Image Loader** | Image cache with memory management | ✅ v0.0.1 |
| **Performance** | Profiler with per-phase timing | ✅ v0.0.1 |
| **Window** | winit-based native window with icon | ✅ v0.0.0 |

## Architecture

The project follows a phased development plan:

| Phase | Area | Status |
|-------|------|--------|
| 1-2 | Project setup & HTTP fetching | ✅ v0.0.1 (Real ureq) |
| 3-4 | HTML parser with DOM construction | ✅ v0.0.1 (Error recovery) |
| 5-6 | CSS parser & style computation | ✅ v0.0.1 (Selectors, specificity) |
| 7-8 | Layout engine with text wrapping | ✅ v0.0.1 (Box model) |
| 9-10 | Text rendering | ✅ v0.0.1 |
| 11-12 | Image loading pipeline | ✅ v0.0.1 |
| 13 | Window manager (winit) | ✅ v0.0.0 |
| 14 | UI framework (Iced) | ✅ v0.0.1 (Full integration) |
| 15-16 | Tab management system | ✅ v0.0.1 |
| 17-18 | Storage (cookies, localStorage) | ✅ v0.0.1 (Persistent) |
| 19-20 | JavaScript engine | ✅ v0.0.1 (Variables, functions) |
| 21-22 | Performance optimization | ✅ v0.0.1 |
| 23-24 | Testing & release documentation | ✅ v0.0.1 (74 tests) |

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

- **winit** 0.29 - Native window creation
- **iced** 0.12 - GUI framework
- **ureq** 2.9 - HTTP/HTTPS client
- **serde** + **serde_json** - Serialization
- **image** 0.24 - Image decoding
- **chrono** 0.4 - Date/time for cookies
- **url** 2.5 - URL parsing
- **log** + **env_logger** - Logging

## Development

See [CONTRIBUTING.md](CONTRIBUTING.md) for guidelines.

## License

MIT License - See LICENSE file for details.
