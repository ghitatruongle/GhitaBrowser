# GhitaBrowser 🦀

![GhitaBrowser Logo](logo.png)

A lightweight Rust browser optimized faster than Chrome, built from scratch with no external reference code or libraries.

## Overview

GhitaBrowser is a complete web browser implementation written entirely in safe Rust (v0.1.2).

### v0.1.2 New Features

- ✅ **Console fix** — No more terminal popup when launching the app (`windows_subsystem = "windows"`)
- ✅ **Dark/Light theme toggle** — Switch between dark and light themes with the 🎨 button in the status bar
- ✅ **Custom brand colors** — Navy + orange brand identity replacing default theme colors
- ✅ **Redesigned tab bar** — Tab pills with orange active indicator, smooth styling
- ✅ **Enhanced toolbar** — Home button, URL clear button, loading spinner, HTTPS padlock indicator
- ✅ **Keyboard shortcuts** — `Ctrl+L` focus URL, `Ctrl+T` new tab, `Ctrl+W` close tab, `F5` reload
- ✅ **DevTools side panel** — JS Console, Storage inspector, Cache stats in a slide-out panel
- ✅ **Loading indicator** — Visual loading spinner and load time tracking
- ✅ **Error page styling** — Formatted error pages with suggestions
- ✅ **Status bar improvements** — Load time, tab count, theme toggle, devtools toggle
- ✅ **Debug diagnostics** — All diagnostic output gated behind `#[cfg(debug_assertions)]` for clean release
- ✅ **Version bump** — Updated across all modules (Cargo.toml, headers, User-Agent strings, HTML templates)

### v0.0.1 Features (Retained)

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
| **Network** | Real HTTP/HTTPS via ureq, ResourceCache with TTL | ✅ v0.1.2 |
| **HTML Parser** | HTML5 tokenizer, error recovery, DOM tree | ✅ v0.1.2 |
| **CSS Parser** | Selectors (tag/class/id), specificity, 20+ properties | ✅ v0.1.2 |
| **Layout** | Box model, text wrapping, block/inline, auto-height | ✅ v0.1.2 |
| **Renderer** | ASCII text rendering of layout tree | ✅ v0.1.2 |
| **JavaScript** | Variables, functions, if/while, console API | ✅ v0.1.2 |
| **Storage** | CookieStore + LocalStorage with serde persistence | ✅ v0.1.2 |
| **UI (Iced)** | Tab bar, navigation, URL bar, status bar, dev tools, theme toggle, keyboard shortcuts | ✅ v0.1.2 |
| **Image Loader** | Image cache with memory management | ✅ v0.1.2 |
| **Performance** | Profiler with per-phase timing | ✅ v0.1.2 |
| **Window** | winit-based native window with icon | ✅ v0.1.2 |

## Keyboard Shortcuts

| Shortcut | Action |
|----------|--------|
| `Ctrl + L` | Focus URL bar |
| `Ctrl + T` | Open new tab |
| `Ctrl + W` | Close current tab |
| `Ctrl + R` / `F5` | Reload current page |
| `Alt + ←` | Go back |
| `Alt + →` | Go forward |

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
| 13 | Window manager (winit) | ✅ v0.0.1 |
| 14 | UI framework (Iced) | ✅ v0.1.2 (Theme, toolbar, devtools, shortcuts) |
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
