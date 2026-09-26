#pragma once

#include <cstdint>
#include <filesystem>

namespace hz {

// Creates `directory` and any missing parents; new directories are 0700.
void make_private_directories(const std::filesystem::path& directory);

// Whether `path` exists, without following a final symlink.
bool exists_nofollow(const std::filesystem::path& path);

// Whether two existing paths are on the same mounted filesystem.
bool same_filesystem(const std::filesystem::path& a, const std::filesystem::path& b);

// Whether `candidate` is `ancestor` or lies below it (both canonical).
bool is_within(const std::filesystem::path& candidate, const std::filesystem::path& ancestor);

// rename(2), refusing to replace an existing destination.
void move_path(const std::filesystem::path& from, const std::filesystem::path& to);

// Deletes the directory tree at `directory`, making read-only directories
// writable as it goes. The workspace marker is removed last, so a tree whose
// deletion was interrupted still identifies which workspace it belonged to.
void remove_tree(const std::filesystem::path& directory);

// Whether a process with this ID exists on this machine.
bool process_alive(std::int64_t pid);

} // namespace hz
