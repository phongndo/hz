{
  description = "hz development environment";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  };

  outputs =
    { nixpkgs, ... }:
    let
      systems = [
        "aarch64-darwin"
        "aarch64-linux"
        "x86_64-darwin"
        "x86_64-linux"
      ];

      forAllSystems =
        function:
        nixpkgs.lib.genAttrs systems (
          system:
          function {
            pkgs = import nixpkgs { inherit system; };
          }
        );
    in
    {
      devShells = forAllSystems (
        { pkgs }:
        {
          default = pkgs.mkShell {
            packages = with pkgs; [
              cargo
              clippy
              coreutils
              curl
              git
              gnutar
              just
              rust-analyzer
              rustc
              rustfmt
              sccache
              zsh
            ];
            shellHook = ''
              hz_dev_root="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"
              hz_dev_bin="$hz_dev_root/target/dev-bin"
              hz_dev_zdotdir="$hz_dev_root/target/dev-zdotdir"

              export HZ_DEV_ROOT="$hz_dev_root"
              export HZ_DEV_BIN="$hz_dev_bin"
              export HZ_DEV_ZDOTDIR="$hz_dev_zdotdir"

              mkdir -p "$HZ_DEV_BIN" "$HZ_DEV_ZDOTDIR"
              cat > "$HZ_DEV_BIN/hz" <<'HZ_DEV_SHIM'
#!/usr/bin/env sh
set -eu

repo="''${HZ_DEV_ROOT:?HZ_DEV_ROOT is not set}"
binary="$repo/target/debug/hz"
stamp="$repo/target/dev-bin/hz.stamp"

needs_build=0
if [ ! -x "$binary" ]; then
  needs_build=1
elif [ "''${HZ_DEV_AUTO_BUILD:-0}" = "1" ]; then
  if [ ! -e "$stamp" ]; then
    needs_build=1
  else
    newer_source="$(find "$repo" \( -path "$repo/target" -o -path "$repo/.git" \) -prune -o -type f \( -name '*.rs' -o -name Cargo.toml -o -name Cargo.lock \) -newer "$stamp" -print | sed -n '1p')"
    if [ -n "$newer_source" ]; then
      needs_build=1
    fi
  fi
fi

if [ "$needs_build" -eq 1 ]; then
  if [ ! -x "$binary" ]; then
    echo "hz dev shim: building hz-cli..." >&2
  else
    echo "hz dev shim: rebuilding hz-cli (HZ_DEV_AUTO_BUILD=1)..." >&2
  fi
  if (cd "$repo" && cargo build -p hz-cli --locked >&2); then
    touch "$stamp"
  else
    exit 1
  fi
fi

exec "$binary" "$@"
HZ_DEV_SHIM
              chmod +x "$HZ_DEV_BIN/hz"
              export PATH="$HZ_DEV_BIN:$PATH"

              cat > "$HZ_DEV_ZDOTDIR/.zshrc" <<'HZ_DEV_ZSHRC'
export PATH="$HZ_DEV_BIN:$PATH"
eval "$(hz shell zsh)"
HZ_DEV_ZSHRC

              if [ -z "''${HZ_DEV_INTERACTIVE_SHELL:-}" ] && [ -t 0 ] && [ -t 1 ] && command -v zsh >/dev/null 2>&1; then
                export HZ_DEV_INTERACTIVE_SHELL=1
                exec env ZDOTDIR="$HZ_DEV_ZDOTDIR" zsh -i
              fi
            '';
          };
        }
      );
    };
}
