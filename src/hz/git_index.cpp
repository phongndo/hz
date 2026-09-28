// Refreshing the stat data cached in a copied Git index. The on-disk format
// is https://git-scm.com/docs/index-format; hz reads it only far enough to
// find each entry's stat data and rewrites nothing else.

#include "hz/detail/digest.hpp"
#include "hz/detail/fd.hpp"
#include "hz/detail/work_queue.hpp"
#include "hz/entry_facts.hpp"
#include "hz/error.hpp"
#include "hz/git.hpp"

#include <algorithm>
#include <atomic>
#include <cctype>
#include <cstdint>
#include <cstring>
#include <fcntl.h>
#include <fstream>
#include <map>
#include <memory>
#include <mutex>
#include <optional>
#include <span>
#include <sstream>
#include <string>
#include <string_view>
#include <sys/stat.h>
#include <unistd.h>
#include <unordered_map>
#include <utility>
#include <vector>

namespace hz::git {

namespace fs = std::filesystem;

namespace {

using Bytes = std::vector<std::uint8_t>;

std::uint32_t load(const Bytes& bytes, std::size_t at) {
    return (static_cast<std::uint32_t>(bytes[at]) << 24) |
           (static_cast<std::uint32_t>(bytes[at + 1]) << 16) |
           (static_cast<std::uint32_t>(bytes[at + 2]) << 8) |
           static_cast<std::uint32_t>(bytes[at + 3]);
}

void store(Bytes& bytes, std::size_t at, std::uint32_t value) {
    bytes[at] = static_cast<std::uint8_t>(value >> 24);
    bytes[at + 1] = static_cast<std::uint8_t>(value >> 16);
    bytes[at + 2] = static_cast<std::uint8_t>(value >> 8);
    bytes[at + 3] = static_cast<std::uint8_t>(value);
}

// One index entry: where its 40 bytes of stat data start, its path and mode,
// and whether Git compares it with the working tree at all.
struct Entry {
    std::size_t stat;
    std::string path;
    std::uint32_t mode;
    bool checked;
};

// Reads the entries of an index whose object IDs are `hash_size` bytes.
class Parser {
  public:
    Parser(const Bytes& bytes, std::size_t hash_size)
        : bytes_(bytes), hash_size_(hash_size), end_(bytes.size() - hash_size) {}

    // Every entry, or nothing if this is not an index hz can rewrite: an
    // unknown version, a split index, or a malformed file.
    [[nodiscard]] std::optional<std::vector<Entry>> entries() {
        if (bytes_.size() < 12 + hash_size_ || std::memcmp(bytes_.data(), "DIRC", 4) != 0) {
            return std::nullopt;
        }
        version_ = load(bytes_, 4);
        if (version_ < 2 || version_ > 4) {
            return std::nullopt;
        }
        std::vector<Entry> result;
        std::size_t at = 12;
        for (std::uint32_t count = load(bytes_, 8); count > 0; --count) {
            if (!entry(at, result)) {
                return std::nullopt;
            }
        }
        if (!plain_extensions(at)) {
            return std::nullopt;
        }
        return result;
    }

  private:
    // Reads the entry at `at` into `result` and moves `at` past it.
    bool entry(std::size_t& at, std::vector<Entry>& result) {
        const std::size_t flags_at = at + 40 + hash_size_;
        if (flags_at + 2 > end_) {
            return false;
        }
        const unsigned flags = (unsigned{bytes_[flags_at]} << 8) | bytes_[flags_at + 1];
        std::size_t name_at = flags_at + 2;
        // Assume-valid and conflict stages are never compared with files.
        bool checked = (flags & 0x8000U) == 0 && (flags & 0x3000U) == 0;
        if ((flags & 0x4000U) != 0) {
            if (version_ < 3 || name_at + 2 > end_) {
                return false;
            }
            // Neither skip-worktree nor intent-to-add entries match a file.
            checked = checked && (bytes_[name_at] & 0x60U) == 0;
            name_at += 2;
        }
        std::string path;
        if (version_ == 4) {
            // A count of bytes to drop from the previous path, then the rest.
            std::size_t drop = 0;
            if (!varint(name_at, drop) || drop > previous_.size()) {
                return false;
            }
            path = previous_.substr(0, previous_.size() - drop);
        }
        const auto name = bytes_.begin() + static_cast<std::ptrdiff_t>(name_at);
        const auto nul = std::find(name, bytes_.begin() + static_cast<std::ptrdiff_t>(end_), 0);
        if (nul == bytes_.begin() + static_cast<std::ptrdiff_t>(end_)) {
            return false;
        }
        path.append(name, nul);
        const auto name_end = static_cast<std::size_t>(nul - bytes_.begin());
        result.push_back(
            {.stat = at, .path = path, .mode = load(bytes_, at + 24), .checked = checked});
        // Before version 4, entries are NUL-padded to a multiple of 8 bytes.
        at = version_ == 4 ? name_end + 1 : at + (((name_end - at) + 8) & ~std::size_t{7});
        previous_ = std::move(path);
        return true;
    }

    // Whether the extensions from `at` end exactly at the checksum and none
    // is a split index link, whose shared index holds more stat data.
    [[nodiscard]] bool plain_extensions(std::size_t at) const {
        while (at + 8 <= end_) {
            if (std::memcmp(&bytes_[at], "link", 4) == 0) {
                return false;
            }
            at += 8 + load(bytes_, at + 4);
        }
        return at == end_;
    }

    // Git's offset varint: seven bits per byte, most significant first, each
    // continuation adding one.
    bool varint(std::size_t& at, std::size_t& value) const {
        if (at >= end_) {
            return false;
        }
        std::uint8_t byte = bytes_[at++];
        value = byte & 0x7fU;
        while ((byte & 0x80U) != 0) {
            if (at >= end_ || value > (SIZE_MAX >> 8)) {
                return false;
            }
            byte = bytes_[at++];
            value = ((value + 1) << 7) | (byte & 0x7fU);
        }
        return true;
    }

    const Bytes& bytes_;
    std::size_t hash_size_;
    std::size_t end_;
    std::uint32_t version_ = 0;
    std::string previous_;
};

// Recomputes the trailing checksum, unless Git was told to skip it
// (index.skipHash), which leaves it zero.
void seal(Bytes& bytes, std::size_t hash_size) {
    const auto end = bytes.end() - static_cast<std::ptrdiff_t>(hash_size);
    if (std::all_of(end, bytes.end(), [](std::uint8_t byte) { return byte == 0; })) {
        return;
    }
    const std::span<const std::uint8_t> content(bytes.data(), bytes.size() - hash_size);
    if (hash_size == 20) {
        std::ranges::copy(detail::sha1(content), end);
    } else {
        std::ranges::copy(detail::sha256(content), end);
    }
}

// The index entry mode Git records for a file with this stat mode.
std::optional<std::uint32_t> entry_mode(mode_t mode) {
    if (S_ISLNK(mode)) {
        return 0120000;
    }
    if (S_ISREG(mode)) {
        return (mode & S_IXUSR) != 0 ? 0100755 : 0100644;
    }
    return std::nullopt;
}

struct Times {
    timespec change;
    timespec modification;
};

Times times_of(const struct stat& info) {
#ifdef __APPLE__
    return {.change = info.st_ctimespec, .modification = info.st_mtimespec};
#else
    return {.change = info.st_ctim, .modification = info.st_mtim};
#endif
}

// Whether the stat data at `at` records `info` as Git stores it, 32 bits of
// each field. Git does not compare device numbers by default.
bool records(const Bytes& bytes, std::size_t at, const struct stat& info) {
    const auto [change, modification] = times_of(info);
    return load(bytes, at) == static_cast<std::uint32_t>(change.tv_sec) &&
           load(bytes, at + 4) == static_cast<std::uint32_t>(change.tv_nsec) &&
           load(bytes, at + 8) == static_cast<std::uint32_t>(modification.tv_sec) &&
           load(bytes, at + 12) == static_cast<std::uint32_t>(modification.tv_nsec) &&
           load(bytes, at + 20) == static_cast<std::uint32_t>(info.st_ino) &&
           load(bytes, at + 28) == static_cast<std::uint32_t>(info.st_uid) &&
           load(bytes, at + 32) == static_cast<std::uint32_t>(info.st_gid) &&
           load(bytes, at + 36) == static_cast<std::uint32_t>(info.st_size);
}

void record(Bytes& bytes, std::size_t at, const struct stat& info) {
    const auto [change, modification] = times_of(info);
    store(bytes, at, static_cast<std::uint32_t>(change.tv_sec));
    store(bytes, at + 4, static_cast<std::uint32_t>(change.tv_nsec));
    store(bytes, at + 8, static_cast<std::uint32_t>(modification.tv_sec));
    store(bytes, at + 12, static_cast<std::uint32_t>(modification.tv_nsec));
    store(bytes, at + 16, static_cast<std::uint32_t>(info.st_dev));
    store(bytes, at + 20, static_cast<std::uint32_t>(info.st_ino));
    store(bytes, at + 28, static_cast<std::uint32_t>(info.st_uid));
    store(bytes, at + 32, static_cast<std::uint32_t>(info.st_gid));
    store(bytes, at + 36, static_cast<std::uint32_t>(info.st_size));
}

// Whether `copy` has the content `original` had, as far as a clone of it
// shows: the same kind, size, and modification time.
bool copied_from(const struct stat& copy, const struct stat& original) {
    const auto first = times_of(copy).modification;
    const auto second = times_of(original).modification;
    return entry_mode(copy.st_mode) == entry_mode(original.st_mode) &&
           copy.st_size == original.st_size && first.tv_sec == second.tv_sec &&
           first.tv_nsec == second.tv_nsec;
}

// Whether an entry modified at the time stored at `at` is racily clean: no
// older than the index `written` then, so its stat data cannot show a later
// change, and Git rereads it.
bool racy(const Bytes& bytes, std::size_t at, const timespec& written) {
    const auto seconds = static_cast<std::uint32_t>(written.tv_sec);
    return seconds < load(bytes, at + 8) ||
           (seconds == load(bytes, at + 8) &&
            static_cast<std::uint32_t>(written.tv_nsec) <= load(bytes, at + 12));
}

// Object IDs are SHA-1 unless the repository says otherwise; 0 for a format
// hz does not know.
std::size_t object_id_size(const fs::path& git_dir) {
    std::ifstream in(git_dir / "config", std::ios::binary);
    std::stringstream text;
    text << in.rdbuf();
    std::string config = text.str();
    std::ranges::transform(config, config.begin(),
                           [](unsigned char c) { return static_cast<char>(std::tolower(c)); });
    if (config.find("objectformat") == std::string::npos) {
        return 20;
    }
    return config.find("sha256") != std::string::npos ? 32 : 0;
}

// The name of an entry within its directory.
const char* name_of(const Entry& entry) {
    const auto slash = entry.path.rfind('/');
    return entry.path.c_str() + (slash == std::string::npos ? 0 : slash + 1);
}

// The directory holding an entry, relative to the top.
std::string directory_of(const Entry& entry) {
    const auto slash = entry.path.rfind('/');
    return slash == std::string::npos ? "." : entry.path.substr(0, slash);
}

// An entry Git would find clean in the source, with that file's stat data.
struct Clean {
    const Entry* entry;
    struct stat original;
};

#ifdef __APPLE__
// Every file and symlink of `directory` by name, read in bulk: on APFS a stat
// call per file costs several times more.
std::map<std::string, struct stat> list_stats(const fs::path& directory) {
    std::map<std::string, struct stat> stats;
    const detail::Fd open_directory(::open(directory.c_str(), O_RDONLY | O_DIRECTORY | O_CLOEXEC));
    if (open_directory.get() < 0) {
        return stats;
    }
    for (auto& facts : list_entry_facts(open_directory.get(), directory, false)) {
        if ((facts.type == DT_REG || facts.type == DT_LNK) && facts.size) {
            stats.emplace(std::move(facts.name), facts.as_stat());
        }
    }
    return stats;
}

// The stat data of each named entry of `directory` that exists.
template <typename Entries, typename Name>
std::vector<std::optional<struct stat>> stat_all(const fs::path& directory, const Entries& entries,
                                                 Name name) {
    const auto stats = list_stats(directory);
    std::vector<std::optional<struct stat>> result;
    for (const auto& entry : entries) {
        const auto found = stats.find(name(entry));
        result.push_back(found == stats.end() ? std::nullopt : std::optional(found->second));
    }
    return result;
}
#else
template <typename Entries, typename Name>
std::vector<std::optional<struct stat>> stat_all(const fs::path& directory, const Entries& entries,
                                                 Name name) {
    std::vector<std::optional<struct stat>> result;
    const detail::Fd open_directory(::open(directory.c_str(), O_RDONLY | O_DIRECTORY | O_CLOEXEC));
    for (const auto& entry : entries) {
        struct stat info{};
        const bool found = open_directory.get() >= 0 && ::fstatat(open_directory.get(), name(entry),
                                                                  &info, AT_SYMLINK_NOFOLLOW) == 0;
        result.push_back(found ? std::optional(info) : std::nullopt);
    }
    return result;
}
#endif

// Whether a sample of `candidates` shows the source's index current. A source
// copied without Git, or changed wholesale since, records stat data none of
// its files match, and checking every entry would only waste time.
bool source_index_current(const fs::path& source, const std::vector<const Entry*>& candidates,
                          const Bytes& bytes) {
    constexpr std::size_t samples = 16;
    const std::size_t step = std::max<std::size_t>(1, candidates.size() / samples);
    for (std::size_t i = 0; i < candidates.size(); i += step) {
        struct stat original{};
        if (::lstat((source / candidates[i]->path).c_str(), &original) == 0 &&
            records(bytes, candidates[i]->stat, original)) {
            return true;
        }
    }
    return false;
}

// Runs `work` for each group on several threads.
template <typename Groups, typename Work> void for_each_group(const Groups& groups, Work work) {
    detail::WorkQueue queue(detail::default_workers());
    for (const auto& group : groups) {
        queue.push([&work, group = &group] { work(group->first, group->second); });
    }
    queue.run();
}

} // namespace

void SourceStats::add(const std::string& relative, const struct stat& info) {
    const std::scoped_lock lock(mutex_);
    entries_.emplace_back(relative, info);
}

struct IndexRefresh::State {
    std::size_t hash_size = 0;
    Bytes bytes;
    std::vector<Entry> entries;
    // Entries Git compares with their files that are not racily clean, by
    // path; empty if the source's index is stale.
    std::unordered_map<std::string_view, const Entry*> candidates;
};

IndexRefresh::IndexRefresh(const fs::path& source) : state_(std::make_unique<State>()) {
    State& state = *state_;
    const fs::path path = source / ".git" / "index";
    state.hash_size = object_id_size(source / ".git");
    const detail::Fd file(::open(path.c_str(), O_RDONLY | O_NOFOLLOW | O_CLOEXEC));
    struct stat index_info{};
    if (state.hash_size == 0 || file.get() < 0 || ::fstat(file.get(), &index_info) != 0) {
        return;
    }
    state.bytes.resize(static_cast<std::size_t>(index_info.st_size));
    if (!std::cmp_equal(::pread(file.get(), state.bytes.data(), state.bytes.size(), 0),
                        state.bytes.size())) {
        state.bytes.clear();
        return;
    }
    auto entries = Parser(state.bytes, state.hash_size).entries();
    if (!entries) {
        return;
    }
    state.entries = std::move(*entries);
    // Git rereads an entry modified no earlier than the index was written, as
    // its stat data cannot show a later change; so must the child.
    const auto written = times_of(index_info).modification;
    std::vector<const Entry*> candidates;
    for (const auto& entry : state.entries) {
        if (entry.checked && !racy(state.bytes, entry.stat, written)) {
            candidates.push_back(&entry);
        }
    }
    if (!source_index_current(source, candidates, state.bytes)) {
        return;
    }
    state.candidates.reserve(candidates.size());
    for (const auto* entry : candidates) {
        state.candidates.emplace(entry->path, entry);
    }
}

IndexRefresh::~IndexRefresh() = default;
IndexRefresh::IndexRefresh(IndexRefresh&&) noexcept = default;
IndexRefresh& IndexRefresh::operator=(IndexRefresh&&) noexcept = default;

std::size_t IndexRefresh::apply(const fs::path& child, const SourceStats& seen) {
    State& state = *state_;
    if (state.candidates.empty()) {
        return 0;
    }
    const fs::path path = child / ".git" / "index";
    const detail::Fd file(::open(path.c_str(), O_RDWR | O_NOFOLLOW | O_CLOEXEC));
    if (file.get() < 0) {
        return 0;
    }
    // The copy must hold exactly the index read: Git may have rewritten the
    // source's since.
    Bytes copied(state.bytes.size() + 1);
    if (!std::cmp_equal(::pread(file.get(), copied.data(), copied.size(), 0), state.bytes.size()) ||
        !std::equal(state.bytes.begin(), state.bytes.end(), copied.begin())) {
        return 0;
    }
    // Entries Git would find clean in the source as the copy read it, by
    // directory. A path read twice keeps its later stat data.
    std::map<const Entry*, struct stat> clean_entries;
    for (const auto& [relative, original] : seen.entries_) {
        const auto found = state.candidates.find(relative);
        if (found != state.candidates.end() &&
            entry_mode(original.st_mode) == found->second->mode &&
            records(state.bytes, found->second->stat, original)) {
            clean_entries.insert_or_assign(found->second, original);
        }
    }
    std::map<std::string, std::vector<Clean>> clean;
    for (const auto& [entry, original] : clean_entries) {
        clean[directory_of(*entry)].push_back({.entry = entry, .original = original});
    }
    // Directories are compared on several threads; each rewrites only its own
    // entries' bytes.
    std::atomic<std::size_t> refreshed = 0;
    for_each_group(clean, [&](const std::string& directory, const std::vector<Clean>& members) {
        const auto copies = stat_all(child / directory, members,
                                     [](const Clean& entry) { return name_of(*entry.entry); });
        for (std::size_t i = 0; i < members.size(); ++i) {
            if (copies[i] && copied_from(*copies[i], members[i].original)) {
                record(state.bytes, members[i].entry->stat, *copies[i]);
                refreshed.fetch_add(1, std::memory_order_relaxed);
            }
        }
    });
    if (refreshed == 0) {
        return 0;
    }
    // In place, as the file keeps its size: nothing uses the child until it is
    // activated, and an interrupted create discards it. Replacing the file
    // instead would make btrfs flush it to disk first.
    seal(state.bytes, state.hash_size);
    if (!std::cmp_equal(::pwrite(file.get(), state.bytes.data(), state.bytes.size(), 0),
                        state.bytes.size())) {
        throw errno_error("write", path);
    }
    return refreshed;
}

} // namespace hz::git
