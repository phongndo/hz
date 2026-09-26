#pragma once

#include <filesystem>

namespace hz {

enum class EntryKind { file, directory, symlink };

// Copies ownership, permission bits, extended attributes, and access and
// modification times from `from` onto `to`, never following symlinks.
// Ownership and attributes the caller is not allowed to set are skipped;
// symlinks keep their own permission bits.
void replay_metadata(const std::filesystem::path& from, const std::filesystem::path& to,
                     EntryKind kind);

} // namespace hz
