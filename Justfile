setup:
    cargo fetch --locked
    cargo build -p hz-cli --locked

dev-init-zsh:
    cargo build -p hz-cli --locked
    @touch "$HOME/.zshrc"
    @grep -qxF 'export PATH="{{ justfile_directory() }}/target/debug:$PATH"' "$HOME/.zshrc" || printf '\n# hz dev binary\nexport PATH="{{ justfile_directory() }}/target/debug:$PATH"\n' >> "$HOME/.zshrc"
    @PATH="{{ justfile_directory() }}/target/debug:$PATH" ./target/debug/hz init zsh
    @echo 'restart your shell or run: source ~/.zshrc'

check:
    rust-analyzer diagnostics .
    cargo fmt --all --check
    cargo clippy --workspace --all-targets --all-features --locked -- -D warnings

build:
    cargo build -p hz-cli --locked

hz *args:
    ./target/debug/hz {{args}}
