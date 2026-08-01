# Contributing to Terminal-Fenster

Thank you for your interest in contributing. This project is experimental; please read
[README.md](README.md) and [RELEASE.md](RELEASE.md) for current limitations before investing
large changes.

## Development setup

Requirements: Rust 1.80+, Node 22.12+, a Kitty-graphics terminal (Ghostty recommended).

```bash
git clone https://github.com/zent7x/terminal-fenster.git
cd terminal-fenster
./install.sh
```

For day-to-day iteration from a checkout:

```bash
tools/dev-terminal-fenster.sh open example.com
```

Always use `terminal-fenster`, not any stale `target/release/blackglass` left over from
before the rename. If in doubt: `terminal-fenster version` prints the binary path; the MCP
subcommand (`terminal-fenster mcp`) exists only on the current binary.

## Running tests

Full release gate (formatting, Rust/JS tests, E2E, packaging smoke):

```bash
tools/release-check.sh
```

Targeted checks:

```bash
cargo test --workspace --locked
cargo clippy --workspace --all-targets -- -D warnings
(cd apps/engine && npm test)
node tests/e2e/input-injection.js
(cd packages/mcp && npm test && npm run test:live)
```

## Pull requests

1. Open an issue for large or architectural changes before coding.
2. Keep PRs focused — one logical change per PR when possible.
3. Run `tools/release-check.sh` (or the relevant subset) before requesting review.
4. Follow existing code style; `cargo fmt` must pass.
5. Do not commit secrets, personal paths, or local audit logs.

## License

By contributing, you agree that your contributions are licensed under the MIT License
(see [LICENSE](LICENSE)).
