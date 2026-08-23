# Changelog

All notable changes to GhitaBrowser are documented here. The project follows
Semantic Versioning.

## [2.0.6] - 2026-08-15

### Added
- Extended Web APIs in `web_runtime.rs`: `ResizeObserver`, `IntersectionObserver`, `requestAnimationFrame`/`cancelAnimationFrame`, `crypto.getRandomValues`, `crypto.subtle`, `matchMedia`, `CSS.supports`.
- Enhanced ECMAScript support in `javascript.rs`: `Promise.allSettled`, `Promise.any`, logical assignment operators (`&&=`, `||=`, `??=`), optimized evaluation scope allocation.
- Kinetic smooth scrolling physics with velocity damping in `ui.rs`.
- Vietnamese IME input normalization and improved Omnibox text editing resilience.
- Layout Box Model accuracy improvements in `layout.rs`: enhanced Flexbox gap/wrap calculations, margin collapsing, multi-line typography wrapping.
- Resilient YouTube playback fallback in `youtube.rs` with multi-client profile discovery and adaptive format selection.
- A conservative request-filtering policy blocks only recognized third-party
  advertising or tracking subresources while preserving documents, local and
  same-site resources, styles, fonts, media and explicit allow rules.
- A representative compatibility corpus, readable fallback and bounded live-site
  probe make the personal release gate require at least 90% usable outcomes with
  no timeouts.
- RAM pressure now uses measured history, response-cache and image-cache bytes,
  with 400 MB/500 MB soft and hard budgets and protected active, pinned, audible,
  internal and error tabs.

### Hardened (release-blocking fixes)
- Inline text boxes are now sized with real glyph advances from the cosmic-text shaping engine (`text_shaper::measure_text_width`), falling back to the heuristic estimator only when the shaping budget rejects a request (`layout.rs`).
- Response finalization (content-type / PDF routing / charset decoding) is unified between the blocking `ureq` path and the async `reqwest` scheduler path through one shared policy (`network::finalize_fetch_response`), with a transport-conformance test proving both paths cannot drift.
- Poisoned-mutex panics are recovered instead of aborting the browser: `messaging::BroadcastChannelBus` and `string_pool` now heal poisoned `Mutex`/`RwLock` state and continue.
- Debug-mode `MAX_CALL_DEPTH` raised from 4 to 8 so JS engine recursion is usable during development while release stays at 32.
- YouTube playback is gated behind an explicit opt-in setting and the SPA/render decision now consults the runtime report (`is_spa_or_js_rendered`) instead of relying on heuristics alone.
- Direct YouTube playback is opt-in and disabled by default for the personal
  release; the normal navigation shell remains the safe fallback.
- CI and local release runners use one Cargo build job with incremental debug
  artifacts disabled, preventing the artifact race and excessive disk use;
  release artifacts are versioned from `Cargo.toml` (`v2.0.6`).
- Personal packaging rejects staged executables whose ProductVersion does not
  match 2.0.6, preventing a stale 2.0.5 binary from being labeled 2.0.6.
- The flaky per-frame timing assertion in the dynamic-rendering acceptance test now measures p95 and an absolute cap instead of asserting every single frame.
- Added `tests/fixtures/web-platform-support-matrix.json` + `tests/web_platform_conformance_test.rs`: a documented support matrix whose unsupported surfaces are enforced to fail closed (TypeError on `window.*`, `undefined` on `navigator.*`).

## [2.0.0] - 2026-08-09

### Added

- Bounded HTML, JavaScript, image, cache and tab-memory processing.
- Sleeping and discarded tabs with recovery support.
- Decoded-image LRU cache and browser memory estimates.
- Release scope, security policy, architecture guide and reproducible toolchain.
- Release-quality integration, persistence and performance test targets.
- Safe local HTML/XHTML/TXT opening and a bounded text-focused PDF reader.
- Process-isolated renderer worker with timeout and checked compressed IPC.
- Flex/grid foundations, form controls and an accessibility tree.
- Limited mutable DOM host bridge, origin storage and bounded event-loop queue.
- Original typed request/cosmetic filter rules with per-site exceptions.
- Percentile telemetry and navigation performance budgets.
- HTTP gzip/Brotli decompression and WHATWG-compatible response charset
  decoding, including legacy `ISO-8859-1` labels used by Google responses.
- Generation-safe Realm heap/GC, deterministic task and microtask queues,
  bounded Promise reaction jobs and WebIDL conversion foundations.
- Independent media-core contracts for MIME/codec parsing, container sniffing,
  range validation, bounded sample queues, audio clock and A/V sync decisions.
- AST-driven JavaScript host capabilities for allowlisted document/element,
  storage and same-origin request discovery operations; string-scanning script
  emulation was removed.
- Phase 10 bounded ECMAScript profile: live lexical closures, block-scoped
  declarations, arrays/objects/prototypes, iterator completion, exceptions,
  arrow/async functions, pending Promise reaction chains and named module
  parse/link/evaluate, backed by a runtime-loaded original corpus and an
  external non-vendored Test262 adapter.
- Phase 11 bounded live DOM: stable node identities; mutable DOM and selector
  APIs; capture/target/bubble dispatch with listener options; form/focus/input
  defaults; and dirty-driven layout, display-list and accessibility refreshes.
- Phase 12 bounded networking Web APIs: queued Fetch settlement,
  Headers/Request/Response/Abort, same-origin and CORS/preflight policy,
  credential and redirect modes, XHR, URL search parameters, timers and
  cross-context storage events, verified by local multi-origin servers.
- Phase 13 bounded application platform: custom-element upgrade and lifecycle
  records, open/closed shadow documents, slot assignment, inert templates,
  module graph integration and an interactive offline SPA hydration gate.
- Phase 14 bounded dynamic rendering: NodeId-keyed incremental cascade cache,
  retained live-document layout/display snapshots, paint-only invalidation,
  position/flex/grid subset, axis-aligned transforms and bounded
  opacity/transform timelines with frame and retained-memory gates.
- Phase 15 bounded media core: real WAVE/PCM and fragmented ISO-BMFF parsing,
  atomic queues/timing and a Windows Media Foundation clear-content gate that
  decodes a synthetic H.264/AAC file into RGBA/PCM buffers.
- Phase 16 headless HTML media/MSE profile: media states/events/controls and
  bounded SourceBuffer append/remove processing over real synthetic fMP4
  fragments, plus JavaScript-visible media/MSE host operations.
- Phase 17 recorded YouTube bootstrap, route, direct-format and player-state
  gates plus browser-owned result/watch shell rendering, bounded in-memory
  Media Foundation AVC/AAC decode, retained-scene video output and a Windows
  WASAPI sink. Live playback remains unclaimed until its black-box gate passes.
- Phase 18 hardening foundations: restricted renderer-worker token, bounded
  Job Object, live cancellation, deterministic parser corpus, navigation/media
  soak, headless installed-artifact smoke and clean-VM gate automation.
- Phase 19 application network core: pooled asynchronous reqwest/rustls
  transport, priorities, HTTP/2, streaming decompression, in-flight socket
  cancellation and shared document/script/style/image/download paths.
- Phase 20 retained scene and compositor: advanced multilingual/RTL text
  shaping with system-font fallback, a bounded CPU path, optional DX12 compute
  compositor, exact CPU/GPU pixel parity and device-loss recovery.

### Changed

- Product positioning now accurately describes a document-focused browser.
- Version reporting is sourced from Cargo package metadata.
- Windows packaging is portable and no longer tied to a developer directory.
- CI validates every target with formatting and warnings treated as errors.
- Unit-test storage is in-memory by default and no longer leaves temporary
  browser profiles on the developer machine.

### Fixed

- Web responses now respect their declared charset instead of rejecting every
  non-UTF-8 byte; gzip and Brotli responses are decoded before text parsing.
- Multibyte Unicode inside scripts no longer corrupts tokenizer byte offsets
  or crashes the renderer worker on pages such as YouTube.
- Renderer failures now replace the tab with a visible error page instead of
  silently leaving the previous New Tab content on screen.

### Security

- Resource size and nesting limits protect the renderer from unbounded input.
- Password storage is not exposed as a supported 2.0 feature until it uses an
  operating-system credential vault.
- Experimental placeholder features are excluded from the release surface.
- The 2026-08-11 locked graph has no RustSec vulnerability. The release gate
  verifies a pinned official `cargo-audit` artifact by SHA-256; eight remaining
  informational transitive warnings are recorded in the dependency audit.

## [1.2.0] - Unreleased development baseline

- Memory saver, memory-pressure tab discard and image-cache improvements.

## [1.1.0]

- Reader-mode and page-fallback improvements.

[2.0.0]: https://github.com/GhitaBrowser/ghitabrowser/releases/tag/v2.0.0
[1.1.0]: https://github.com/GhitaBrowser/ghitabrowser/releases/tag/v1.1.0
