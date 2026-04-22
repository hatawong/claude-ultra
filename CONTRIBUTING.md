# Contributing to Claude Ultra

Thank you for your interest in contributing! This guide covers the development setup and contribution workflow.

## Important: Source Build Limitations

The public repository contains a **stub implementation** of the closed-source `http/` crate. Source builds compile and pass type checks, but **cannot proxy real API requests** (TLS fingerprint and billing hash are placeholder values). This is by design.

Contributions to the Manager UI, Gateway routing logic, account management, and other non-stub components are welcome.

## Development Setup

### Prerequisites

- macOS 12+
- [Rust](https://rustup.rs/) 1.94.1+
- [Bun](https://bun.sh) 1.x
- [Google Chrome](https://www.google.com/chrome/)

### Getting Started

```bash
git clone https://github.com/hatawong/claude-ultra.git
cd claude-ultra

# Install frontend dependencies
bun install

# Verify Rust compiles
cd src-tauri
cargo check --lib

# Run tests
cargo test --test header_order

# Type check frontend
cd ..
bunx tsc --noEmit

# Start dev server (UI only)
bun run tauri:dev
```

### Project Structure

```
claude-ultra/
├── http/              # Stub HTTP client (closed-source boundary)
│   └── src/           # Public API preserved, implementation stubbed
├── src-tauri/         # Rust backend (Tauri commands, Gateway, proxy)
│   ├── src/
│   │   ├── gateway/   # API gateway (handler, builder, security)
│   │   ├── proxy/     # Proxy pool and allocator
│   │   ├── modules/   # Account manager, CLI client, monitor
│   │   ├── models/    # Account, config data models
│   │   └── commands/  # Tauri IPC commands
│   └── tests/         # Integration tests
├── src/               # React frontend
│   ├── components/    # UI components
│   ├── pages/         # Route pages
│   ├── stores/        # Zustand state stores
│   ├── hooks/         # Custom React hooks
│   ├── services/      # Tauri IPC service layer
│   └── locales/       # i18n translations (12 languages)
└── docs/              # Documentation and screenshots
```

## Pull Request Guidelines

1. **Fork** the repository and create a branch from `main`
2. **Focus** each PR on a single concern
3. **Test** your changes:
   - `cargo check --lib` must pass
   - `bunx tsc --noEmit` must pass
   - `cargo test --test header_order` must pass
4. **Don't modify** `http/src/` stub files unless fixing a type compatibility issue
5. **i18n**: If adding user-facing strings, add keys to all 12 locale files in `src/locales/`

## Code Style

- **Rust**: Follow standard `rustfmt` conventions
- **TypeScript/React**: Functional components, hooks-based state
- **CSS**: Tailwind utility classes
- **Comments**: English only in source code (Chinese strings in UI are intentional for i18n)

## What Not to Submit

- Real API keys, tokens, or credentials
- Changes to `http/` stub that would enable real API requests
- Closed-source implementation details that live only in the official build

## Questions?

Open an [issue](https://github.com/hatawong/claude-ultra/issues) for questions or discussion.
