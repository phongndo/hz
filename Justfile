setup:
    cargo fetch --locked
    cargo build -p hz-cli --locked

check:
    rust-analyzer diagnostics .
    cargo fmt --all --check
    cargo clippy --workspace --all-targets --all-features --locked -- -D warnings

ci-check: ci-rust ci-integration ci-performance ci-workflows

ci-rust:
    scripts/ci/rust

ci-integration:
    scripts/ci/integration

ci-performance:
    scripts/ci/performance smoke

ci-workflows:
    actionlint -color

# Run hk checks (equivalent to pre-commit hook steps)
hk-check:
    mise x hk -- hk check

# Run full hk checks including slow steps (requires --profile slow or --profile ci)
hk-check-full:
    mise x hk -- hk check --profile slow

test:
    cargo test --workspace --all-targets --all-features --locked

build:
    cargo build -p hz-cli --locked

hooks:
    mise x hk -- hk validate
    @echo 'Global hk hooks are active (hk-pre-commit, hk-pre-push)'

hz *args:
    cargo build -p hz-cli --locked
    ./target/debug/hz {{args}}

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
