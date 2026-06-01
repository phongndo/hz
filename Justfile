setup:
    cargo fetch --locked
    cargo build -p hz-cli --locked

check:
    rust-analyzer diagnostics .
    cargo fmt --all --check
    cargo clippy --workspace --all-targets --all-features --locked -- -D warnings

build:
    cargo build -p hz-cli --locked

smoke: smoke-cli smoke-zsh

smoke-cli:
    cargo build -p hz-cli --locked
    ./target/debug/hz --help >/dev/null
    ./target/debug/hz shell zsh >/dev/null
    ./target/debug/hz shell bash >/dev/null
    ./target/debug/hz shell fish >/dev/null

smoke-zsh:
    zsh scripts/smoke-zsh

smoke-curl-install version="latest":
    scripts/smoke-curl-install {{version}}

hz *args:
    ./target/debug/hz {{ args }}
