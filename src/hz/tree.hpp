#pragma once

#include "hz/clone.hpp"

#include <filesystem>
#include <functional>
#include <string>
#include <sys/stat.h>

namespace hz {

struct CopyTreeOptions {
    CopyMode mode = CopyMode::cow;
    // Called with each entry's path relative to the source root. Returning
    // true omits the entry and, for a directory, everything below it. Called
    // concurrently from several threads.
    std::function<bool(const std::filesystem::path&)> skip;
    // Threads copying directories in parallel; 0 chooses a default.
    unsigned workers = 0;
    // On macOS in cow mode, clone the whole tree with one call while scanning
    // the source, then fix up where that differs from walking it. Worth it
    // when `skip` omits no directory; otherwise the clone is discarded and
    // the tree walked.
    bool clone_whole_tree = false;
    // Called with the path of each source regular file and symlink, relative
    // to the source root, and its stat data as the copy read it. Called
    // concurrently from several threads, possibly more than once per path.
    std::function<void(const std::string&, const struct stat&)> observe;
    // Called once, on the calling thread, when every file of the copy is
    // final; directory metadata may still be replayed after it returns.
    std::function<void()> files_copied;
};

// Copies the directory `from` to the new directory `to` on the same
// filesystem: directories, regular files, symlinks (never followed), and hard
// links, which are recreated as hard links within the copy. Permission bits,
// ownership where permitted, extended attributes, and timestamps are replayed
// onto every entry, each directory once everything below it is copied.
// Entries are opened relative to their directory; only symlink metadata and
// later links of a hard-linked file use full paths. On macOS a subtree whose
// clone would come out the same is cloned with one clonefile instead.
// Sockets, fifos, and devices are unsupported entries. On any failure nothing
// is left at `to` and the error propagates.
void copy_tree(const std::filesystem::path& from, const std::filesystem::path& to,
               const CopyTreeOptions& options);

} // namespace hz
