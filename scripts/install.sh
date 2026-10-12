#!/bin/sh
# Installs an hz release:
#   curl -fsSL https://raw.githubusercontent.com/phongndo/hz/main/scripts/install.sh | sh
# Each release goes in its own directory under HZ_RELEASES_DIR, and the command is a symlink to
# it in HZ_INSTALL_DIR. Every release ships this script; `hz update` reruns it.
set -eu

version=${HZ_VERSION:-latest}
install_dir=${HZ_INSTALL_DIR:-"$HOME/.local/bin"}
releases_dir=${HZ_RELEASES_DIR:-"$HOME/.local/share/hz/releases"}

fail() {
  printf 'hz install: %s\n' "$*" >&2
  exit 1
}
for tool in curl tar mktemp; do
  command -v "$tool" >/dev/null 2>&1 || fail "missing required command: $tool"
done
if command -v sha256sum >/dev/null 2>&1; then
  checksum() { sha256sum "$1" | awk '{print $1}'; }
elif command -v shasum >/dev/null 2>&1; then
  checksum() { shasum -a 256 "$1" | awk '{print $1}'; }
else
  fail "sha256sum or shasum is required to verify the download"
fi

case "$(uname -s)-$(uname -m)" in
  Darwin-arm64) target=aarch64-apple-darwin ;;
  Linux-x86_64 | Linux-amd64) target=x86_64-unknown-linux-musl ;;
  Linux-aarch64 | Linux-arm64) target=aarch64-unknown-linux-musl ;;
  *) fail "releases are built for Apple Silicon macOS and x86_64 or ARM64 Linux; elsewhere, build hz from source" ;;
esac
case "$install_dir:$releases_dir" in
  /*:/*) ;;
  *) fail "HZ_INSTALL_DIR and HZ_RELEASES_DIR must be absolute paths" ;;
esac
case "$install_dir$releases_dir" in
  *"
"*) fail "HZ_INSTALL_DIR and HZ_RELEASES_DIR must not contain newlines" ;;
esac

# Replace only a command this installer put there: Homebrew's, Nix's, mise's or your own stays.
destination="$install_dir/hz"
if [ -e "$destination" ] || [ -L "$destination" ]; then
  [ -L "$destination" ] || fail "$destination already exists; remove it or set HZ_INSTALL_DIR"
  case "$(readlink "$destination")" in
    "$releases_dir"/*/bin/hz) ;;
    *) fail "$destination belongs to another installation; update that one with the tool that installed it, or set HZ_INSTALL_DIR" ;;
  esac
fi

if [ "$version" = latest ]; then
  # GitHub redirects to the latest release's page, which ends in its tag.
  latest=$(curl -fsSLI -o /dev/null -w '%{url_effective}' https://github.com/phongndo/hz/releases/latest) ||
    fail "could not look up the latest release"
  case "$latest" in
    */releases/tag/*) version=${latest##*/} ;;
    *) fail "no release has been published yet" ;;
  esac
fi
case "$version" in
  v*) tag=$version ;;
  *) tag="v$version" ;;
esac
printf '%s\n' "$tag" | LC_ALL=C grep -Eq '^v[0-9]+\.[0-9]+\.[0-9]+$' || fail "invalid release version: $version"

# `hz update` says which release is running.
if [ "${HZ_CURRENT_RELEASE:-}" = "$tag" ] && [ -x "$destination" ]; then
  printf 'hz %s is already installed at %s\n' "$tag" "$destination"
  exit 0
fi

package="hz-$tag-$target"
asset="$package.tar.gz"
base_url=${HZ_RELEASE_BASE_URL:-"https://github.com/phongndo/hz/releases/download/$tag"}
temporary=$(mktemp -d)
pending=""
cleanup() {
  rm -rf "$temporary"
  if [ -n "$pending" ]; then rm -rf "$pending"; fi
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

printf 'Installing hz %s for %s\n' "$tag" "$target"
curl -fL --progress-bar --retry 3 "$base_url/$asset" -o "$temporary/$asset" || fail "could not download $asset"
curl -fsSL --retry 3 "$base_url/$asset.sha256" -o "$temporary/$asset.sha256" || fail "could not download the checksum"
expected=$(awk -v name="$asset" 'NF == 2 && $2 == name && length($1) == 64 && $1 !~ /[^0-9a-f]/ {print $1}' "$temporary/$asset.sha256")
if [ -z "$expected" ] || [ "$(checksum "$temporary/$asset")" != "$expected" ]; then
  fail "checksum verification failed"
fi
tar -xzf "$temporary/$asset" -C "$temporary"
[ -x "$temporary/$package/bin/hz" ] || fail "the archive has no bin/hz"

# On Linux the default is inside hz's own data directory, which hz keeps private.
(umask 077 && mkdir -p "$releases_dir")
mkdir -p "$install_dir"
pending=$(mktemp -d "$releases_dir/$tag-$target.XXXXXX")
cp -R "$temporary/$package/." "$pending/"
# The receipt is how `hz update` knows this installer owns the installation.
mkdir -p "$pending/share/hz"
printf 'release=%s\ninstall_dir=%s\nreleases_dir=%s\n' "$tag" "$install_dir" "$releases_dir" \
  >"$pending/share/hz/installation"
"$pending/bin/hz" --version >/dev/null || fail "the downloaded hz does not run on this system"
# Renamed into place, so the command is never missing or half-written.
ln -s "$pending/bin/hz" "$install_dir/.hz.$$"
mv -f "$install_dir/.hz.$$" "$destination"
installed=$pending
pending=""

# A running hz keeps its executable open, so earlier releases can go now. Only directories with
# this installer's receipt are removed; HZ_RELEASES_DIR may hold other things.
for release in "$releases_dir"/v*; do
  if [ "$release" != "$installed" ] && [ -f "$release/share/hz/installation" ]; then
    rm -rf "$release"
  fi
done

printf '\nInstalled hz %s at %s\n' "$tag" "$destination"
case ":$PATH:" in
  *":$install_dir:"*) ;;
  *) printf 'Add %s to your PATH to run it.\n' "$install_dir" ;;
esac
printf 'Get started in a project with: hz init\n'
printf 'Let hz change directory with: hz install zsh (or bash or fish)\n'
printf 'Update with: hz update\n'
