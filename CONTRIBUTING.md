# Contributing to GhitaBrowser

Welcome! We're excited to have you help build this Rust-native browser. 🦀

## 📜 Code of Conduct

Please adhere to the [Rust Code of Conduct](https://www.rust-lang.org/p/code-of-conduct) in all interactions.

## 🛠 Getting Started

1. **Fork** the repository
2. **Clone** your fork locally
3. **Create** a new branch for your feature/fix
4. **Commit** your changes with clear messages
5. **Push** to your fork
6. **Open** a Pull Request

## 🎯 Development Guidelines

### Coding Standards

- All code must be written in **100% Rust** (no C/C++ exceptions unless absolutely necessary)
- Follow **Rust style guidelines** (`cargo fmt`)
- Maintain **100% coverage** for new features (unit tests required)
- Use **`#[allow(...)]` judiciously** - prefer safe alternatives
- **Document** all public APIs with doc comments
- Include **examples** where appropriate

### Performance Requirements

- Memory usage must be optimized (avoid unnecessary allocations)
- Startup time under 100ms achievable
- Zero-copy paths for critical rendering operations

### Testing

- Write unit tests for all new functions
- Integration tests for core browser functionality
- Performance benchmarks for optimization verification

## 🤝 Submitting Changes

1. Fork and create branch: `git checkout -b feature/my-feature`
2. Commit changes: `git commit -m "Add my feature"`
3. Push to branch: `git push origin feature/my-feature`
4. Open Pull Request against `main` branch

### PR Checklist

- [ ] Tests added/updated
- [ ] Code formatted (`cargo fmt`)
- [ ] No Clippy warnings (`cargo clippy`)
- [ ] Documentation updated
- [ ] Version bump if applicable

## 📞 Need Help?

Join our Discord or open an issue on GitHub! We're happy to help contributors get started.

---

*Thank you for helping us build the future of browsers!* 🔥