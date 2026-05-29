## Rust Workspace Rules

- Preserve the existing crate boundaries unless the pull request explicitly changes architecture.
- Prefer clear error propagation over panics in library crates.
- Keep CLI behavior stable unless the pull request documents the user-facing change.
- Require focused verification for the touched crate or command path.
