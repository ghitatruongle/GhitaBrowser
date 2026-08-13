# GhitaBrowser

![GhitaBrowser logo](logo.png)

GhitaBrowser 2.0 is a lightweight, Windows-first, document-focused browser
written in safe Rust. It provides a native tabbed interface and a bounded
HTML/CSS rendering pipeline without embedding Chromium, Gecko or WebKit.

## What 2.0 supports

- HTTP/HTTPS navigation, redirects, cookies, cache and readable error pages.
- Gzip/Brotli response decoding and declared legacy web charset support.
- A custom HTML parser, CSS subset, layout engine and pixel display list.
- Flex/grid foundations, basic forms and a bounded accessibility tree.
- Safe local HTML/XHTML/TXT opening and a text-focused PDF reader (`Ctrl+O`).
- Isolated renderer worker with bounded compressed IPC and timeout recovery.
- Decoded raster images with download, dimension and memory limits.
- Tabs, closed-tab restore, tab search, horizontal/vertical tabs and Task Manager.
- History, bookmarks, downloads, settings, search and reader fallback.
- Incognito tabs that do not use persistent cookies, response cache, history or
  download history.
- Sleeping/discarded tabs, image LRU and memory-pressure protection.
- Project-authored request/cosmetic filtering with per-site exceptions.
- Limited inline DOM mutations, same-origin fetch discovery, origin storage and
  a bounded event-loop foundation.

## Deliberate limitations

GhitaBrowser is not a full modern web-platform implementation. Its JavaScript
engine supports a bounded language subset but not a complete DOM or Web APIs.
Sites requiring unsupported SPA hydration, DRM, live video output, WebRTC,
service workers or browser extensions may show a readable fallback instead of
the interactive app. The bounded headless media/MSE profile and its exact
release-UI limits are documented in `docs/media-conformance.md` and
`docs/html-media-mse-conformance.md`.

PiP, Web Capture, sidebar apps, quick notes, split-screen and password autofill
are not advertised as 2.0 features because their experimental modules do not yet
meet the release criteria. See [the exact 2.0 scope](docs/2.0.0-scope.md) and
[security boundaries](SECURITY.md).

## Build and test

Requirements:

- Windows x64.
- Rust 1.97.1, installed automatically when using rustup in this directory.

```powershell
cargo build --locked
cargo run --release --locked

cargo fmt --all -- --check
cargo check --all-targets --locked
cargo test --all-targets --locked
cargo clippy --all-targets --all-features --locked -- -D warnings

# Run the complete non-installing release gate
powershell -ExecutionPolicy Bypass -File .\tools\release-gate.ps1
```

Run performance benchmarks with:

```powershell
cargo bench --locked
```

## Packaging

Create the release executable and portable ZIP from any checkout location:

```powershell
powershell -ExecutionPolicy Bypass -File .\packaging\package.ps1
```

If Inno Setup 6 is installed, the same script also builds the per-user setup
executable. Artifacts are written to `dist/`, which is intentionally ignored by
Git.

For a local developer install without creating a package:

```powershell
cargo build --release --locked
powershell -ExecutionPolicy Bypass -File .\install.ps1
```

## Common shortcuts

| Shortcut | Action |
|---|---|
| `Ctrl+L` or `F6` | Focus the address bar |
| `Ctrl+O` | Open a local HTML, XHTML, TXT or PDF document |
| `Ctrl+T` / `Ctrl+W` | Open / close a tab |
| `Ctrl+Shift+T` | Reopen the last closed tab |
| `Ctrl+Tab` | Select the next tab |
| `Ctrl+Shift+A` | Search open tabs |
| `Shift+Esc` | Open Task Manager |
| `Ctrl+H` / `Ctrl+J` | History / downloads |
| `Ctrl+F` | Find in page |
| `F12` | Open the built-in diagnostics panel |

## Project documents

- [Architecture](docs/architecture.md)
- [Engine and media roadmap](docs/engine-media-roadmap.md)
- [2.0 release scope](docs/2.0.0-scope.md)
- [2.0 dependency and license audit](docs/dependency-audit.md)
- [Changelog](CHANGELOG.md)
- [Security policy](SECURITY.md)
- [Clean-room policy](docs/clean-room-policy.md)
- [Specification provenance](docs/specification-provenance.md)
- [Third-party notices](THIRD_PARTY_NOTICES.md)
- [Contributing](CONTRIBUTING.md)

GhitaBrowser's original code is proprietary. Separately licensed dependencies
are listed in [Third-party notices](THIRD_PARTY_NOTICES.md).
