#pragma once

#include <filesystem>
#include <sys/stat.h>

namespace hz {

// The mode bits hz copies: permissions, set-ID, and sticky.
inline constexpr mode_t permission_bits = 07777;

// The ownership and permission bits a destination entry had when hz created
// it, so replay can skip writes that would change nothing.
struct CreatedAs {
    uid_t uid;
    gid_t gid;
    mode_t mode; // permission bits only
};

// Replays ownership, extended attributes, permission bits, and access and
// modification times from the open `source` (described by `info`) onto
// `destination`, a file or directory hz just created with owner write
// permission. Ownership and attributes the caller is not allowed to set are
// skipped. On macOS the ACL is copied too. The paths only label errors.
void replay_metadata(int source, int destination, const struct stat& info, const CreatedAs& created,
                     const std::filesystem::path& from, const std::filesystem::path& to);

// The same for a symlink, by path and never following it. Symlinks keep their
// own permission bits.
void replay_symlink_metadata(const std::filesystem::path& from, const std::filesystem::path& to,
                             const struct stat& info);

} // namespace hz
