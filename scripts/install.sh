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

allow_unverified() {
  case "${HZ_ALLOW_UNVERIFIED:-}" in
    1 | true | TRUE | yes | YES)
      return 0
      ;;
    *)
      return 1
      ;;
  esac
}

need curl
need tar
need install

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
checksum="$asset.sha256"
if curl -fL "$base_url/$checksum" -o "$checksum"; then
  if command -v shasum >/dev/null 2>&1; then
    shasum -a 256 -c "$checksum"
  elif command -v sha256sum >/dev/null 2>&1; then
    sha256sum -c "$checksum"
  elif allow_unverified; then
    echo "hz install: warning: shasum or sha256sum not found; skipping checksum verification" >&2
  else
    echo "hz install: shasum or sha256sum not found; set HZ_ALLOW_UNVERIFIED=1 to skip checksum verification" >&2
    exit 1
  fi
elif allow_unverified; then
  echo "hz install: warning: checksum file not available; skipping checksum verification" >&2
else
  echo "hz install: checksum file not available; set HZ_ALLOW_UNVERIFIED=1 to skip checksum verification" >&2
  exit 1
fi

tar -xzf "$asset"
install_source="$package/hz"
if [ ! -d "$package" ] || [ ! -x "$install_source" ]; then
  echo "hz install: extracted archive does not contain executable $install_source" >&2
  exit 1
fi

mkdir -p "$install_dir"
install -m 755 "$install_source" "$install_dir/$binary"

echo "installed $binary $tag to $install_dir/$binary"
echo "run: $binary --version"
