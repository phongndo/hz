#pragma once

#include <filesystem>

namespace hz {

enum class CopyMode {
    cow,  // share data blocks with the source; fail if the filesystem cannot
    copy, // duplicate the bytes
};

// Creates the new regular file `to` with the contents and permission bits of
// `from`. In cow mode the data blocks are shared until either file is written.
// Throws Error(cow_unavailable) when the filesystem refuses to clone; any other
// failure is an io error. Nothing is left at `to` on failure.
void clone_file(const std::filesystem::path& from, const std::filesystem::path& to, CopyMode mode);

// Whether copy-on-write cloning works for files created inside `directory`.
// Creates and removes two probe files there.
bool probe_clone_support(const std::filesystem::path& directory);

} // namespace hz
