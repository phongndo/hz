#!/usr/bin/env sh
set -eu

repo="${HZ_REPO:-phongndo/hz}"
version="${HZ_VERSION:-latest}"
install_dir="${HZ_INSTALL_DIR:-$HOME/.local/bin}"
binary="${HZ_BINARY:-hz}"

need() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "hz install: missing required command: $1" >&2
    exit 1
  fi
}

need curl
need tar

case "$(uname -s)" in
  Darwin)
    platform="apple-darwin"
    ;;
  Linux)
    platform="unknown-linux-gnu"
    ;;
  *)
    echo "hz install: unsupported OS: $(uname -s)" >&2
    exit 1
    ;;
esac

case "$(uname -m)" in
  arm64 | aarch64)
    arch="aarch64"
    ;;
  x86_64 | amd64)
    arch="x86_64"
    ;;
  *)
    echo "hz install: unsupported architecture: $(uname -m)" >&2
    exit 1
    ;;
esac

if [ "$version" = "latest" ]; then
  tag="$(
    curl -fsSL "https://api.github.com/repos/$repo/releases/latest" \
      | sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' \
      | head -n 1
  )"
  if [ -z "$tag" ]; then
    echo "hz install: could not resolve latest release for $repo" >&2
    exit 1
  fi
else
  case "$version" in
    v*) tag="$version" ;;
    *) tag="v$version" ;;
  esac
fi

target="$arch-$platform"
package="hz-$tag-$target"
asset="$package.tar.gz"
base_url="https://github.com/$repo/releases/download/$tag"
tmp_dir="$(mktemp -d)"

cleanup() {
  rm -rf "$tmp_dir"
}
trap cleanup EXIT INT TERM

cd "$tmp_dir"
curl -fL "$base_url/$asset" -o "$asset"
curl -fL "$base_url/$asset.sha256" -o "$asset.sha256"

if command -v shasum >/dev/null 2>&1; then
  shasum -a 256 -c "$asset.sha256"
elif command -v sha256sum >/dev/null 2>&1; then
  sha256sum -c "$asset.sha256"
else
  echo "hz install: warning: shasum or sha256sum not found; skipping checksum verification" >&2
fi

tar -xzf "$asset"
mkdir -p "$install_dir"
install -m 755 "$package/hz" "$install_dir/$binary"

echo "installed $binary $tag to $install_dir/$binary"
echo "run: $binary --version"
