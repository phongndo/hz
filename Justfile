setup:
    cargo fetch --locked
    cargo build -p hz-cli --locked

install-hooks:
    git config core.hooksPath .githooks
    printf 'Git hooks installed from .githooks\n'

check:
    rust-analyzer diagnostics .
    cargo fmt --all --check
    cargo clippy --workspace --all-targets --all-features --locked -- -D warnings

build:
    cargo build -p hz-cli --locked

smoke: smoke-cli smoke-zsh smoke-bench smoke-installer-update

smoke-cli:
    cargo build -p hz-cli --locked
    ./target/debug/hz --help >/dev/null
    ./target/debug/hz shell zsh >/dev/null
    ./target/debug/hz shell bash >/dev/null
    ./target/debug/hz shell fish >/dev/null

smoke-zsh:
    zsh scripts/smoke-zsh

smoke-bench:
    cargo build -p hz-cli --locked
    cargo run -p hz-bench --locked -- cmd --hz target/debug/hz --worktrees 2 --warmup 0 --iterations 1 --json >/dev/null

smoke-installer-update version="latest":
    scripts/smoke-installer-update {{version}}

smoke-curl-install version="latest":
    scripts/smoke-curl-install {{version}}

smoke-update version="latest":
    scripts/smoke-update {{version}}

hz *args:
    ./target/debug/hz {{ args }}
