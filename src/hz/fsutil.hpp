#pragma once

#include "hz/detail/fd.hpp"

#include <chrono>
#include <cstdint>
#include <filesystem>
#include <string>
#include <string_view>
#include <vector>

namespace hz {

// Serializes lifecycle mutations across processes, waiting for another holder
// up to a limit. Refused inside a hook while the hz process running it, which
// may hold the lock, is still running with the same registry.
detail::Fd lock_operations(const std::filesystem::path& data_directory,
                           std::chrono::seconds wait = std::chrono::seconds(120));

// Names the lease an hz process holds while its hooks run, in their
// environment.
inline constexpr const char* hook_parent_variable = "HZ_HOOK_PARENT";

// A lease marks one workspace as busy with a long operation, such as copying
// or deleting it, that runs without the operation lock. The lease lasts as
// long as the returned descriptor, so a crash releases it. Acquire and check
// workspace leases only while holding the operation lock; a lease named by a
// fresh ULID has no competitors and needs no lock. Release a lease under the
// lock unless nothing else could acquire it meanwhile.
// With `wait`, blocks while another process briefly checks the lease instead
// of failing; use it only for a fresh ULID nobody else can hold.
detail::Fd acquire_lease(const std::filesystem::path& data_directory, std::string_view id,
                         bool wait = false);
enum class Lease {
    absent, // never taken, or ended cleanly
    free,   // its file remains: the process holding it died
    held,
};
Lease lease_state(const std::filesystem::path& data_directory, std::string_view id);
// Removes the file of a lease nobody holds, left by a crash; keeps held ones.
void remove_free_lease(const std::filesystem::path& data_directory, std::string_view id);

// Ends a lease held through `lease` and removes its file.
void release_lease(const std::filesystem::path& data_directory, std::string_view id,
                   detail::Fd lease);

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

// One entry of a directory listing. `type` is a DT_* value, never DT_UNKNOWN;
// DT_FIFO stands for any special file.
struct DirectoryEntry {
    std::string name;
    unsigned char type;
};

// Every entry of the open directory except `.` and `..`, read completely
// before returning. `path` only labels errors.
std::vector<DirectoryEntry> read_directory(int directory, const std::filesystem::path& path);

// Opens the directory `name` in `parent`, first making it accessible to its
// owner (0700) if they cannot read it. Never follows a symlink swapped in for
// it. `path` only labels errors.
detail::Fd open_directory_as_owner(int parent, const char* name, const std::filesystem::path& path);

// Whether a process with this ID exists on this machine.
bool process_alive(std::int64_t pid);

// Threads for tree work, shared by the hz processes of one registry. Copies
// and deletions contend on filesystem locks, so concurrent commands each
// running a full set of threads finish later than if they shared one set.
// Takes whichever of the shared slots are free, without waiting; with none
// free, work still runs on one thread. Slots last as long as this object.
class WorkerSlots {
  public:
    explicit WorkerSlots(const std::filesystem::path& data_directory);

    // Threads the holder may use: at least one.
    [[nodiscard]] unsigned count() const noexcept;

  private:
    std::vector<detail::Fd> held_;
};

// Deletes the directory tree at `directory`, making read-only directories
// writable as it goes. The workspace marker is removed last, so a tree whose
// deletion was interrupted still identifies which workspace it belonged to.
// `workers` threads delete in parallel; 0 chooses a default.
void remove_tree(const std::filesystem::path& directory, unsigned workers = 0);

} // namespace hz
