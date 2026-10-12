# Installation

Releases are built for macOS 14 or newer on Apple Silicon, and for x86_64 and
ARM64 Linux. The Linux executable is static, so it runs on any distribution,
NixOS and Alpine included. `hz git` and `hz hg` run the `git` and `hg` on your
`PATH`.

| Method | Install | Update |
| --- | --- | --- |
| [Homebrew](#homebrew) | `brew install phongndo/tap/hz-cli` | `brew upgrade hz-cli` |
| [Install script](#install-script) | `curl -fsSL …/install.sh \| sh` | `hz update` |
| [mise](#mise) | `mise use -g github:phongndo/hz` | `mise upgrade github:phongndo/hz` |
| [Nix](#nix) | `nix profile add github:phongndo/hz` | `nix profile upgrade hz` |

Each method manages its own copy, so update hz with the one that installed it.
The registry of your workspaces lives elsewhere (`~/.local/share/hz` or
`$XDG_DATA_HOME/hz` on Linux, `~/Library/Application Support/hz` on macOS, or
`$HZ_DATA_DIR`) and stays when you reinstall or switch methods.

## Homebrew

```sh
brew install phongndo/tap/hz-cli
```

The formula is `hz-cli` because homebrew-core's `hz` is another project, the
command-line tool of CloudWeGo's Go HTTP framework, and installs an `hz` command
too; Homebrew won't install both. The tap picks up a new release within six
hours of its publication.

## Install script

```sh
curl -fsSL https://raw.githubusercontent.com/phongndo/hz/main/scripts/install.sh | sh
```

The script needs `curl`, `tar`, and `sha256sum` or `shasum`. It downloads the
release for your system, checks its SHA-256 checksum, unpacks it under
`~/.local/share/hz/releases`, and links `~/.local/bin/hz` to it. If your shell
doesn't find `hz` afterwards, add that directory to your `PATH`:

```sh
export PATH="$HOME/.local/bin:$PATH"
```

Update to the latest release, or to a particular one:

```sh
hz update
hz update 0.10.0
```

`hz update` reruns the install script that came with your release, in the same
directories, and does nothing if that release is already installed. A failed
download or checksum leaves the installed hz as it was. `hz update` refuses to
touch an hz that Homebrew, mise or Nix installed, and the script won't replace
an `hz` command that it didn't put there.

Environment variables choose a release or other directories:

| Variable | Default | Meaning |
| --- | --- | --- |
| `HZ_VERSION` | the latest release | the release to install, such as `0.10.0` |
| `HZ_INSTALL_DIR` | `~/.local/bin` | where the `hz` command goes |
| `HZ_RELEASES_DIR` | `~/.local/share/hz/releases` | where releases are unpacked, one directory each |
| `HZ_RELEASE_BASE_URL` | the GitHub release | a mirror holding the release's archives and `.sha256` files |

```sh
curl -fsSL https://raw.githubusercontent.com/phongndo/hz/main/scripts/install.sh |
  HZ_VERSION=0.10.0 sh
```

`hz update` remembers the directories. Both must be absolute paths. On Linux the
default releases directory is inside hz's own data directory, which the script
creates private, as hz does; `HZ_DATA_DIR` moves hz's registry but not the
releases. The script removes only the release directories it made.

hz 0.9 and earlier were a different implementation whose install script put a
plain `hz` file in `~/.local/bin`. This script won't replace it, and hz 0.10
doesn't read 0.9's workspaces (see [the registry](design.md#registry)). Rename
or remove the old command when you no longer need it, then run the script.

## mise

[mise](https://mise.jdx.dev) installs the release archive from GitHub:

```sh
mise use -g github:phongndo/hz
```

Pin a version with `github:phongndo/hz@0.10.0`, globally or in a project's
`mise.toml`.

## Nix

Nix builds hz from source with the dependencies pinned in `flake.lock`, running
the test suite on the way. Try it without installing:

```sh
nix run github:phongndo/hz -- --help
```

Install it into your profile:

```sh
nix profile add github:phongndo/hz
```

Or add the flake to a NixOS, nix-darwin or Home Manager configuration:

```nix
{
  inputs.hz.url = "github:phongndo/hz";

  # In a NixOS or nix-darwin module:
  environment.systemPackages = [ inputs.hz.packages.${pkgs.stdenv.hostPlatform.system}.default ];
  # Or in Home Manager:
  home.packages = [ inputs.hz.packages.${pkgs.stdenv.hostPlatform.system}.default ];
}
```

and update it with `nix flake update hz` and a rebuild.

## From source

The Nix build is the tested one: in a checkout, `nix build` puts hz in
`./result`. Elsewhere you need a C++23 compiler and standard library, CMake
3.28+, Ninja, and the development packages for CLI11, toml++, nlohmann_json and
SQLite 3, plus Catch2 3 unless you pass `-DHZ_BUILD_TESTS=OFF`.

```sh
cmake --preset release -DHZ_BUILD_TESTS=OFF
cmake --build --preset release
./build/release/hz --version
```

## Removal

- Homebrew: `brew uninstall hz-cli`
- Install script: `rm ~/.local/bin/hz` and `rm -r ~/.local/share/hz/releases`,
  or the directories you chose
- mise: `mise unuse -g github:phongndo/hz`
- Nix: `nix profile remove hz`, or remove the flake input and package

Remove the line `hz install` added to your shell's startup file too. Workspaces
are ordinary directories: remove the ones you no longer want with `hz rm` and
`hz gc` first, then delete the registry directory above to remove hz's data.
