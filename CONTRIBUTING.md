# Contributing to Teleemix

Thank you for your interest in contributing! Here's how to get involved.

## Ways to contribute

- **Bug reports** — Open an issue describing the problem, steps to reproduce, and your setup
- **Feature requests** — Open an issue describing the feature and why it would be useful
- **Bug fixes** — Fork the repo, fix the bug, and open a pull request
- **Documentation** — Improvements to the README, wiki, or code comments are always welcome

## Getting started

### Prerequisites

- Rust (latest stable via [rustup](https://rustup.rs/))
- Docker + Docker Compose
- A running deemix instance for testing

### Local development

```bash
git clone https://github.com/asupersheep/teleemix.git
cd teleemix
cp .env.example .env
# fill in your .env values
cargo run
```

### Building

```bash
cargo build --release
```

### Building the Docker image locally

```bash
docker build -t teleemix:local .
```

## Pull request guidelines

1. Fork the repository and create a branch from `dev` (not `main`)
2. Make your changes
3. Test that the bot starts and basic functionality works
4. Open a pull request against the `dev` branch
5. Describe what you changed and why

## Branch structure

| Branch | Purpose |
|---|---|
| `main` | Stable releases only |
| `dev` | Active development, PRs go here |

## Commit messages

Use clear, descriptive commit messages:
- `Fix: ...` for bug fixes
- `Feature: ...` for new features
- `Docs: ...` for documentation changes
- `Refactor: ...` for code restructuring

## Code style

- Follow standard Rust conventions (`cargo fmt` and `cargo clippy`)
- Keep functions focused and reasonably short
- Add comments for non-obvious logic

## Reporting security issues

Please do **not** open public issues for security vulnerabilities. See [SECURITY.md](SECURITY.md) for the responsible disclosure process.
