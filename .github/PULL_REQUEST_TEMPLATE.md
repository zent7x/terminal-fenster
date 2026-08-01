## What changed

Describe the user-visible outcome and why this is the smallest appropriate change.

## Verification

- [ ] `cargo fmt --all -- --check`
- [ ] Relevant Rust/JavaScript tests pass
- [ ] `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] Website production build passes when `website/` changed
- [ ] I tested in a real graphics terminal when terminal rendering or input changed

List the exact commands and any manual terminal used:

```text

```

## Safety and compatibility

- [ ] No secrets, private URLs, personal paths, or unredacted browser data are included
- [ ] User/page-controlled text remains sanitized before reaching the terminal
- [ ] Terminal modes, images, sockets, profiles, and child processes clean up on failure
- [ ] Documentation and known-gap claims were updated where behavior changed
