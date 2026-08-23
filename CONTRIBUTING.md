# Contributing to GhitaBrowser

## Development workflow

1. Use the Rust toolchain defined in `rust-toolchain.toml`.
2. Keep changes inside the documented 2.0 product and security boundaries.
3. Add unit tests for pure logic and integration tests for complete pipelines.
4. Run the release checks before submitting a change.

```powershell
cargo fmt --all -- --check
cargo check --all-targets --locked
cargo test --all-targets --locked
cargo clippy --all-targets --all-features --locked -- -D warnings

# Centralized tiers with one build job and JSON metrics
.\tools\test.ps1 -Tier fast
.\tools\test.ps1 -Tier release
.\tools\test.ps1 -Tier full
```

The clean-workspace budget for `target/debug` is below 8 GB. Inspect the latest
file in `dist/build-metrics` when changing build profiles; do not delete another
developer's artifacts to satisfy the budget.

Changes affecting parsing, layout, networking, storage or untrusted input should
also add a bounded/adversarial regression test. Performance-sensitive changes
should run `cargo bench --locked` and report the before/after result.

## Code guidelines

- Keep production Rust safe; isolate and justify any future unsafe code.
- Prefer explicit size, time, depth and count limits for untrusted input.
- Do not expose a UI control until its end-to-end behavior and failure state are
  implemented and tested.
- Keep asynchronous responses bound to the originating tab and navigation
  sequence.
- Preserve incognito isolation and avoid persisting private state.
- Document public APIs and user-visible limitations.
- Follow the [clean-room policy](docs/clean-room-policy.md): do not inspect,
  copy, translate or adapt code from another browser engine.
- Record new standards inputs in
  [specification provenance](docs/specification-provenance.md).
- Do not add a dependency until its license and release-artifact notices have
  been reviewed.

## Pull-request checklist

- Formatting, checks, tests and Clippy pass with the locked dependency graph.
- New behavior has deterministic tests that do not depend on public internet.
- Documentation, changelog and version metadata are consistent when relevant.
- No generated `target/` or `dist/` artifact is included.
- Security-sensitive changes explain their trust boundary and resource limits.
- The license metadata audit passes: `pwsh ./tools/audit-licenses.ps1`.

Please follow the [Rust Code of Conduct](https://www.rust-lang.org/policies/code-of-conduct).
