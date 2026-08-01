# Security policy

## Supported versions

Terminal-Fenster is experimental pre-1.0 software. Security fixes land on `main`; there are no
long-term support branches yet.

## Reporting a vulnerability

**Please do not open public GitHub issues for security vulnerabilities.**

Report security issues privately via [GitHub Security Advisories](https://github.com/zent7x/terminal-fenster/security/advisories/new)
for the `zent7x/terminal-fenster` repository.

Include:

- A description of the issue and impact
- Steps to reproduce
- Affected versions or commits
- Any suggested fix, if you have one

We aim to acknowledge reports within a few business days. Coordinated disclosure is appreciated.

## Scope notes

The MCP server and engine control socket are local trust boundaries. Reports about missing
hardening on unreleased features documented in `artifacts/swarm/` are welcome but may be
triaged as known gaps until [RELEASE.md](RELEASE.md) gates are met.
