# Installation

Releases are built for macOS 14 or newer on Apple Silicon, and for x86_64 and
ARM64 Linux. The Linux executable is static, so it runs on any distribution,
NixOS and Alpine included. `hz git` and `hz hg` run the `git` and `hg` on your
`PATH`.

| Method | Install |
| --- | --- |
| [Homebrew](#homebrew) | `brew install phongndo/tap/hz-cli` |
| [Install script](#install-script) | `curl -fsSL …/install.sh \| sh` |
| [mise](#mise) | `mise use -g github:phongndo/hz` |
| [Nix](#nix) | `nix profile add github:phongndo/hz` |

Each method manages its own copy, and [`hz update`](#updating) updates the one
you run with the method that installed it. The registry of your workspaces lives
elsewhere (`~/.local/share/hz` or `$XDG_DATA_HOME/hz` on Linux,
`~/Library/Application Support/hz` on macOS, or `$HZ_DATA_DIR`) and stays when
you reinstall or switch methods.

## Homebrew

```sh
brew install phongndo/tap/hz-cli
```

The formula is `hz-cli` because homebrew-core's `hz` is another project, the
command-line tool of CloudWeGo's Go HTTP framework, and installs an `hz` command
too; Homebrew won't install both. The tap picks up a new release within an
hour of its publication.

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

A failed download or checksum leaves the installed hz as it was, and the script
won't replace an `hz` command that it didn't put there.

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

Both directories must be absolute paths, and `hz update` keeps using them. On
Linux the default releases directory is inside hz's own data directory, which
the script creates private, as hz does; `HZ_DATA_DIR` moves hz's registry but not
the releases. The script removes only the release directories it made.

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

Or add the flake to the inputs of a NixOS, nix-darwin or Home Manager
configuration:

```nix
inputs.hz.url = "github:phongndo/hz";
```

and its package to a NixOS or nix-darwin module:

```nix
environment.systemPackages = [ inputs.hz.packages.${pkgs.stdenv.hostPlatform.system}.default ];
```

or to your Home Manager configuration:

```nix
home.packages = [ inputs.hz.packages.${pkgs.stdenv.hostPlatform.system}.default ];
```

[Updating](#updating) covers how each of these updates.

## Updating

```sh
hz update
```

`hz update` works out how the hz you ran was installed and updates it the same
way:

- Homebrew: `brew upgrade hz-cli`.
- Install script: it reruns the install script that came with your release, in
  the same directories, which does nothing if the release is already installed.
- mise: `mise upgrade github:phongndo/hz`, which stays within a version you
  pinned.
- Nix profile: `nix profile upgrade` with the name `nix profile list` gives hz in
  your default profile.

Before running Homebrew, mise or Nix, `hz update` prints the command, such as
`Updating with: brew upgrade hz-cli`, then hands over to it: the output and exit
status are that command's.

Only the install script installs a particular release, such as `0.10.0`:

```sh
hz update 0.10.0
```

Homebrew installs only the latest release. With mise, pin a version with
`mise use -g github:phongndo/hz@0.10.0`; with Nix, remove hz from your profile
and add `github:phongndo/hz/v0.10.0`. Given a version, `hz update` says so
instead of updating.

`hz update` can't update an hz from a NixOS, nix-darwin or Home Manager
configuration, from a profile other than your default one, or from `nix run` or
a development shell. For a configuration, update its flake input with
`nix flake update hz` and rebuild. Nor can `hz update` update an hz you built
from source: rebuild it, or reinstall it with one of the methods above.

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
