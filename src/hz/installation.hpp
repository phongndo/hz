#pragma once

#include <filesystem>
#include <optional>
#include <string>
#include <string_view>

namespace hz {

// Where scripts/install.sh put hz, read from the receipt it leaves in each
// release (share/hz/installation). Homebrew, mise, Nix, and source builds
// leave none.
struct Installation {
    std::filesystem::path install_dir;  // holds the `hz` symlink
    std::filesystem::path releases_dir; // holds one directory per release
};

// Nothing unless the receipt names both directories as absolute paths.
std::optional<Installation> parse_installation(std::string_view receipt);

// What installed the running hz, which decides how `hz update` updates it.
enum class Installer {
    install_script, // its receipt is beside the executable
    homebrew,       // <prefix>/Cellar/<formula>/<version>/...
    mise,           // under mise's installs directory
    nix,            // in the Nix store: a profile, a system configuration, or `nix run`
    other,          // a source build, or anything else
};

struct InstalledBy {
    Installer installer = Installer::other;
    std::filesystem::path homebrew_prefix; // the directory holding Cellar
    std::string homebrew_formula;
    std::filesystem::path nix_store_path; // /nix/store/<hash>-<name>
};

// Classifies the running executable's canonical path. `mise_data_dir` is
// MISE_DATA_DIR, empty when unset; `has_receipt` says whether the install
// script's receipt beside the executable parsed.
InstalledBy installed_by(const std::filesystem::path& executable, std::string_view mise_data_dir,
                         bool has_receipt);

// The name of the element in `nix profile list --json` output whose store
// paths include `store_path`. Nothing for unreadable output or no match.
std::optional<std::string> nix_profile_element(std::string_view profile_list,
                                               const std::filesystem::path& store_path);

} // namespace hz
