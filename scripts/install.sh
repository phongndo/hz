#!/usr/bin/env sh
set -eu

repo="${HZ_REPO:-phongndo/hz}"
version="${HZ_VERSION:-latest}"
install_dir="${HZ_INSTALL_DIR:-$HOME/.local/bin}"
binary="${HZ_BINARY:-hz}"
action="${HZ_INSTALL_ACTION:-install}"
case "$action" in
  install | update)
    ;;
  *)
    action="install"
    ;;
esac

print_plan() {
  if [ "$action" = "update" ]; then
    printf 'Updating %s\n' "$binary"
    if [ -n "${HZ_CURRENT_VERSION:-}" ]; then
      printf '  from: v%s\n' "${HZ_CURRENT_VERSION#v}"
    fi
    printf '  to:   %s\n' "$tag"
    printf '  path: %s\n' "$install_dir/$binary"
  else
    printf 'Installing %s\n' "$binary"
    printf '  version: %s\n' "$tag"
    printf '  target:  %s\n' "$target"
    printf '  path:    %s\n' "$install_dir/$binary"
  fi
  printf '\n'
}

print_success() {
  printf '✓ %s\n' "$1"
}

print_path_hint() {
  active_binary="$(command -v "$binary" 2>/dev/null || true)"
  if [ "$active_binary" = "$install_dir/$binary" ]; then
    return 0
  fi

  printf '\n%s was installed, but your shell may not find it yet.\n' "$binary"
  printf 'Add this to your shell profile:\n\n'
  printf '  export PATH="%s:$PATH"\n' "$install_dir"
}

managed_install_name() {
  case "$1" in
    /opt/homebrew/* | /home/linuxbrew/.linuxbrew/* | */.linuxbrew/* | */Cellar/*)
      printf 'Homebrew'
      return 0
      ;;
    */.cargo/bin | */.cargo/bin/*)
      printf 'Cargo'
      return 0
      ;;
    */.local/share/mise/shims | */.local/share/mise/shims/* | */.local/share/mise/installs | */.local/share/mise/installs/* | */.mise/shims | */.mise/shims/* | */.mise/installs | */.mise/installs/*)
      printf 'mise'
      return 0
      ;;
    /nix/store/* | */.nix-profile/bin | */.nix-profile/bin/* | */.local/state/nix/profile/bin | */.local/state/nix/profile/bin/* | /run/current-system/sw/bin | /run/current-system/sw/bin/*)
      printf 'Nix'
      return 0
      ;;
    */.asdf/shims | */.asdf/shims/* | */.asdf/installs | */.asdf/installs/*)
      printf 'asdf'
      return 0
      ;;
  esac

  return 1
}

refuse_managed_path() {
  path="$1"
  label="$2"
  manager="$(managed_install_name "$path" || true)"
  if [ -z "$manager" ]; then
    return 0
  fi

  echo "hz $action: refusing to write to $manager-managed $label: $path" >&2
  echo "hz $action: choose an unmanaged directory, for example: $HOME/.local/bin" >&2
  exit 1
}

refuse_managed_install_dir() {
  install_target="$install_dir/$binary"

  refuse_managed_path "$install_dir" "install directory"
  refuse_managed_path "$install_target" "install target"

  if [ -L "$install_target" ] && command -v readlink >/dev/null 2>&1; then
    link_target="$(readlink "$install_target" || true)"
    if [ -n "$link_target" ]; then
      case "$link_target" in
        /*) resolved_target="$link_target" ;;
        *) resolved_target="$install_dir/$link_target" ;;
      esac
      refuse_managed_path "$resolved_target" "install target symlink"
    fi
  fi

  if [ -e "$install_target" ] && command -v realpath >/dev/null 2>&1; then
    real_target="$(realpath "$install_target" 2>/dev/null || true)"
    if [ -n "$real_target" ]; then
      refuse_managed_path "$real_target" "install target"
    fi
  fi
}

curl_download() {
  curl -fsSL "$1" -o "$2"
}

need() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "hz $action: missing required command: $1" >&2
    exit 1
  fi
}

allow_unverified() {
  case "${HZ_ALLOW_UNVERIFIED:-}" in
    1 | [Tt][Rr][Uu][Ee] | [Yy][Ee][Ss])
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
    echo "hz $action: unsupported OS: $(uname -s)" >&2
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
    echo "hz $action: unsupported architecture: $(uname -m)" >&2
    exit 1
    ;;
esac

target="$arch-$platform"
refuse_managed_install_dir

if [ "$version" = "latest" ]; then
  tag="$(
    curl -fsSL "https://api.github.com/repos/$repo/releases/latest" \
      | sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' \
      | head -n 1
  )"
  if [ -z "$tag" ]; then
    echo "hz $action: could not resolve latest release for $repo" >&2
    exit 1
  fi
else
  case "$version" in
    v*) tag="$version" ;;
    *) tag="v$version" ;;
  esac
fi

package="hz-$tag-$target"
asset="$package.tar.gz"
base_url="https://github.com/$repo/releases/download/$tag"
tmp_dir="$(mktemp -d)"

print_plan

cleanup() {
  rm -rf "$tmp_dir"
}
trap cleanup EXIT INT TERM

cd "$tmp_dir"
curl_download "$base_url/$asset" "$asset"
checksum="$asset.sha256"
if curl_download "$base_url/$checksum" "$checksum"; then
  if command -v shasum >/dev/null 2>&1; then
    if [ "$action" = "update" ]; then
      shasum -a 256 -c "$checksum" >/dev/null
    else
      shasum -a 256 -c "$checksum"
    fi
  elif command -v sha256sum >/dev/null 2>&1; then
    if [ "$action" = "update" ]; then
      sha256sum -c "$checksum" >/dev/null
    else
      sha256sum -c "$checksum"
    fi
  elif allow_unverified; then
    echo "hz $action: warning: shasum or sha256sum not found; skipping checksum verification" >&2
  else
    echo "hz $action: shasum or sha256sum not found; set HZ_ALLOW_UNVERIFIED=1 to skip checksum verification" >&2
    exit 1
  fi
elif allow_unverified; then
  echo "hz $action: warning: checksum file not available; skipping checksum verification" >&2
else
  echo "hz $action: checksum file not available; set HZ_ALLOW_UNVERIFIED=1 to skip checksum verification" >&2
  exit 1
fi

tar -xzf "$asset"
install_source="$package/hz"
if [ ! -d "$package" ] || [ ! -x "$install_source" ]; then
  echo "hz $action: extracted archive does not contain executable $install_source" >&2
  exit 1
fi

mkdir -p "$install_dir"
install -m 755 "$install_source" "$install_dir/$binary"

if [ "$action" = "update" ]; then
  print_success "Updated $binary to $tag"
else
  print_success "Installed $binary $tag"
  printf 'Run: %s\n' "$binary"
  print_path_hint
fi
