setup:
    cargo fetch --locked
    cargo build -p hz-cli --locked

init-zsh:
    zsh scripts/dev-zsh

check:
    rust-analyzer diagnostics .
    cargo fmt --all --check
    cargo clippy --workspace --all-targets --all-features --locked -- -D warnings

build:
    cargo build -p hz-cli --locked

hz *args:
    ./target/debug/hz {{ args }}

dev:
    zsh scripts/dev-zsh --enter
