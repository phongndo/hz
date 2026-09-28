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

// Fills the new, empty regular file open as `destination` from the file open
// as `source`: shares blocks in cow mode, duplicates bytes in copy mode. The
// paths only label errors. On macOS only copy mode is available here; clone
// files there with clone_at.
void copy_contents(int source, int destination, CopyMode mode, const std::filesystem::path& from,
                   const std::filesystem::path& to);

#ifdef __APPLE__
// clonefile(2) of the entry `name` in `source_directory` to the same name in
// `destination_directory`. A regular file's clone carries the source's mode
// (without set-ID bits), extended attributes, ACL, and timestamps. A
// directory is cloned with everything below it.
void clone_at(int source_directory, const char* name, int destination_directory,
              const std::filesystem::path& from);

// clonefile(2) of the directory `from`, with everything below it, to the new
// path `to`. Below the top, entries lose their ACLs and take the group of
// `to`'s parent, and every directory gets the current time.
void clone_tree(const std::filesystem::path& from, const std::filesystem::path& to);
#endif

// Whether copy-on-write cloning works for files created inside `directory`.
// Creates and removes two probe files there.
bool probe_clone_support(const std::filesystem::path& directory);

} // namespace hz
