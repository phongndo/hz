#include "hz/tree.hpp"

#include "hz/error.hpp"
#include "hz/metadata.hpp"

#include <format>
#include <map>
#include <sys/stat.h>
#include <utility>

namespace hz {

namespace fs = std::filesystem;

namespace {

struct stat lstat_entry(const fs::path& path) {
    struct stat info{};
    if (::lstat(path.c_str(), &info) != 0) {
        throw errno_error("stat", path);
    }
    return info;
}

// Creates `path` readable only by the owner; group and others must not see a
// partially populated copy. The source's bits are replayed once it is full.
void make_private_directory(const fs::path& path) {
    if (::mkdir(path.c_str(), 0700) != 0) {
        throw errno_error("create directory", path);
    }
    // A restrictive umask can strip owner bits from mkdir's mode.
    if (::chmod(path.c_str(), 0700) != 0) {
        throw errno_error("set permissions", path);
    }
}

class TreeCopier {
  public:
    explicit TreeCopier(const CopyTreeOptions& options) : options_(options) {}

    void copy_directory(const fs::path& source, const fs::path& destination,
                        const fs::path& relative) {
        for (const auto& entry : fs::directory_iterator(source)) {
            const fs::path name = entry.path().filename();
            const fs::path child_relative = relative / name;
            if (options_.skip && options_.skip(child_relative)) {
                continue;
            }
            copy_entry(entry.path(), destination / name, child_relative);
        }
    }

  private:
    void copy_entry(const fs::path& source, const fs::path& destination, const fs::path& relative) {
        const struct stat info = lstat_entry(source);
        switch (info.st_mode & S_IFMT) {
        case S_IFDIR:
            make_private_directory(destination);
            copy_directory(source, destination, relative);
            replay_metadata(source, destination, EntryKind::directory);
            break;
        case S_IFREG:
            copy_regular_file(source, destination, info);
            break;
        case S_IFLNK:
            fs::create_symlink(fs::read_symlink(source), destination);
            replay_metadata(source, destination, EntryKind::symlink);
            break;
        default:
            throw Error(ErrorKind::unsupported_entry,
                        std::format("cannot copy special file {}", source.string()));
        }
    }

    void copy_regular_file(const fs::path& source, const fs::path& destination,
                           const struct stat& info) {
        if (info.st_nlink > 1) {
            const auto key = std::pair{info.st_dev, info.st_ino};
            if (auto existing = hard_links_.find(key); existing != hard_links_.end()) {
                fs::create_hard_link(existing->second, destination);
                return;
            }
            hard_links_.emplace(key, destination);
        }
        clone_file(source, destination, options_.mode);
        replay_metadata(source, destination, EntryKind::file);
    }

    const CopyTreeOptions& options_;
    std::map<std::pair<dev_t, ino_t>, fs::path> hard_links_;
};

} // namespace

void copy_tree(const fs::path& from, const fs::path& to, const CopyTreeOptions& options) {
    if (fs::exists(fs::symlink_status(to))) {
        throw Error(ErrorKind::invalid_path,
                    std::format("destination already exists: {}", to.string()));
    }
    make_private_directory(to);
    try {
        TreeCopier copier(options);
        copier.copy_directory(from, to, fs::path{});
        replay_metadata(from, to, EntryKind::directory);
    } catch (...) {
        std::error_code ignored;
        fs::remove_all(to, ignored);
        throw;
    }
}

} // namespace hz
