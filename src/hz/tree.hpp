#pragma once

#include "hz/clone.hpp"

#include <filesystem>
#include <functional>

namespace hz {

struct CopyTreeOptions {
    CopyMode mode = CopyMode::cow;
    // Called with each entry's path relative to the source root. Returning
    // true omits the entry and, for a directory, everything below it. Called
    // concurrently from several threads.
    std::function<bool(const std::filesystem::path&)> skip;
    // Threads copying directories in parallel; 0 chooses a default.
    unsigned workers = 0;
};

// Copies the directory `from` to the new directory `to` on the same
// filesystem: directories, regular files, symlinks (never followed), and hard
// links, which are recreated as hard links within the copy. Permission bits,
// ownership where permitted, extended attributes, and timestamps are replayed
// onto every entry, each directory once everything below it is copied.
// Entries are opened relative to their directory; only symlink metadata and
// later links of a hard-linked file use full paths.
// Sockets, fifos, and devices are unsupported entries. On any failure nothing
// is left at `to` and the error propagates.
void copy_tree(const std::filesystem::path& from, const std::filesystem::path& to,
               const CopyTreeOptions& options);

} // namespace hz
