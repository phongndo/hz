setup:
    cargo fetch --locked
    cargo build -p hz-cli --locked

check:
    rust-analyzer diagnostics .
    cargo fmt --all --check
    cargo clippy --workspace --all-targets --all-features --locked -- -D warnings

build:
    cargo build -p hz-cli --locked

smoke: smoke-cli smoke-zsh smoke-installer-update

smoke-cli:
    cargo build -p hz-cli --locked
    ./target/debug/hz --help >/dev/null
    ./target/debug/hz shell zsh >/dev/null
    ./target/debug/hz shell bash >/dev/null
    ./target/debug/hz shell fish >/dev/null

smoke-zsh:
    zsh scripts/smoke-zsh

smoke-installer-update version="latest":
    scripts/smoke-installer-update {{version}}

smoke-curl-install version="latest":
    scripts/smoke-curl-install {{version}}

smoke-update version="latest":
    scripts/smoke-update {{version}}

smoke-mise version="latest":
    scripts/smoke-mise {{version}}

homebrew-formula version dist="dist":
    scripts/render-homebrew-formula {{version}} {{dist}}

hz *args:
    ./target/debug/hz {{ args }}
