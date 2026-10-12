#pragma once

#include <filesystem>
#include <optional>
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

} // namespace hz
