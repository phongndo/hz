#pragma once

#ifdef __APPLE__

#include <array>
#include <cstdint>
#include <ctime>
#include <dirent.h>
#include <filesystem>
#include <optional>
#include <string>
#include <sys/stat.h>
#include <sys/types.h>
#include <vector>

namespace hz {

// An entry's metadata as getattrlistbulk(2) reports it, which on APFS costs a
// fraction of a stat call per entry. Attributes the filesystem does not
// report leave `type` DT_UNKNOWN.
struct EntryFacts {
    std::string name;
    unsigned char type = DT_UNKNOWN; // DT_REG, DT_DIR, DT_LNK, DT_FIFO for any special file
    dev_t device = 0;
    ino_t inode = 0;
    uid_t uid = 0;
    gid_t gid = 0;
    mode_t mode = 0;                 // permission bits only
    std::uint32_t links = 0;         // not for directories
    std::optional<off_t> size;       // not for directories, where reported
    bool acl = false;                // read only when asked for
    std::array<timespec, 2> times{}; // access, modification
    timespec change{};

    // The same as a struct stat, as far as it goes.
    [[nodiscard]] struct stat as_stat() const;
};

// The facts of the open directory `directory` itself. `path` labels errors.
EntryFacts entry_facts(int directory, const std::filesystem::path& path);

// The facts of every entry of the open directory `directory`, with whether
// each has an ACL if `acl`. `path` labels errors.
std::vector<EntryFacts> list_entry_facts(int directory, const std::filesystem::path& path,
                                         bool acl);

} // namespace hz

#endif
