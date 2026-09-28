#pragma once

#include <filesystem>
#include <string_view>

namespace hz {

// How a workspace's files were produced from its source.
enum class Strategy {
    snapshot, // one filesystem snapshot of the whole source
    clone,    // walked, cloning each file or clean subtree
    copy,     // walked, copying bytes
};

std::string_view to_string(Strategy strategy);

// Whether `directory` can be copied by snapshotting it: it is the root of a
// btrfs subvolume and holds no other subvolume, which a snapshot would leave
// as an empty directory. Always false where hz has no snapshot support.
bool can_snapshot(const std::filesystem::path& directory);

// Creates `to`, which must not exist, as a writable snapshot of `from`, for
// which can_snapshot holds. Returns false, creating nothing, if the caller
// may not snapshot it.
bool snapshot(const std::filesystem::path& from, const std::filesystem::path& to);

} // namespace hz
