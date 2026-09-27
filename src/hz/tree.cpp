#include "hz/tree.hpp"

#include "hz/detail/fd.hpp"
#include "hz/detail/work_queue.hpp"
#include "hz/error.hpp"
#include "hz/fsutil.hpp"
#include "hz/metadata.hpp"

#include <atomic>
#include <cerrno>
#include <dirent.h>
#include <fcntl.h>
#include <format>
#include <map>
#include <memory>
#include <mutex>
#include <string>
#include <sys/stat.h>
#include <unistd.h>
#include <utility>
#include <vector>

namespace hz {

namespace fs = std::filesystem;

namespace {

constexpr mode_t private_mode = 0700;

struct stat stat_fd(int fd, const fs::path& path) {
    struct stat info{};
    if (::fstat(fd, &info) != 0) {
        throw errno_error("stat", path);
    }
    return info;
}

detail::Fd open_directory_at(int directory, const char* name, const fs::path& path) {
    return detail::Fd::open_at(directory, name, O_RDONLY | O_DIRECTORY | O_NOFOLLOW, 0,
                               "open directory", path);
}

CreatedAs created_as(const struct stat& info) {
    return {.uid = info.st_uid,
            .gid = info.st_gid,
            .mode = static_cast<mode_t>(info.st_mode & permission_bits)};
}

Error unsupported(const fs::path& path) {
    return {ErrorKind::unsupported_entry,
            std::format("cannot copy special file {}", path.string())};
}

// A directory being copied. Its destination stays private (0700) while it is
// populated; its own metadata is replayed once everything below is copied.
struct Directory {
    detail::Fd source{-1};
    detail::Fd destination{-1};
    struct stat info{};
    CreatedAs created{};
    fs::path relative; // below both roots
    std::shared_ptr<Directory> parent;
    // Its own listing plus each subdirectory and file batch not yet finished.
    std::atomic<std::size_t> pending{1};
};

class TreeCopier {
  public:
    TreeCopier(fs::path from, fs::path to, const CopyTreeOptions& options)
        : from_(std::move(from)), to_(std::move(to)), options_(options),
          queue_(options.workers == 0 ? detail::default_workers() : options.workers) {}

    void run(detail::Fd source, detail::Fd destination) {
        auto root = std::make_shared<Directory>();
        root->info = stat_fd(source.get(), from_);
        root->created = created_as(stat_fd(destination.get(), to_));
        root->source = std::move(source);
        root->destination = std::move(destination);
        queue_.push([this, root] { list(root); });
        queue_.run();
    }

  private:
    // Creates and lists the subdirectory `name` of `parent`.
    void enter(const std::shared_ptr<Directory>& parent, const std::string& name) {
        auto directory = std::make_shared<Directory>();
        directory->relative = parent->relative / name;
        directory->parent = parent;
        const fs::path from = from_ / directory->relative;
        const fs::path to = to_ / directory->relative;
        directory->source = open_directory_at(parent->source.get(), name.c_str(), from);
        directory->info = stat_fd(directory->source.get(), from);
        if (::mkdirat(parent->destination.get(), name.c_str(), private_mode) != 0) {
            throw errno_error("create directory", to);
        }
        directory->destination =
            open_directory_as_owner(parent->destination.get(), name.c_str(), to);
        directory->created = created_as(stat_fd(directory->destination.get(), to));
        // A restrictive umask can strip owner bits from mkdir's mode, and an
        // inherited default ACL can open it to others.
        if (directory->created.mode != private_mode) {
            if (::fchmod(directory->destination.get(), private_mode) != 0) {
                throw errno_error("set permissions", to);
            }
            directory->created.mode = private_mode;
        }
        list(directory);
    }

    void list(const std::shared_ptr<Directory>& directory) {
        std::vector<std::string> files;
        for (auto& entry : read_directory(directory->source.get(), from_ / directory->relative)) {
            copy_entry(directory, std::move(entry), files);
            if (files.size() == detail::file_batch) {
                directory->pending.fetch_add(1, std::memory_order_relaxed);
                queue_.push(
                    [this, directory, batch = std::move(files)] { copy_files(directory, batch); });
                files.clear();
            }
        }
        copy_files(directory, files);
    }

    // Copies regular files of `directory`, then counts the batch finished.
    void copy_files(const std::shared_ptr<Directory>& directory,
                    const std::vector<std::string>& names) {
        for (const auto& name : names) {
            copy_file(*directory, name.c_str(), directory->relative / name);
        }
        finish(directory);
    }

    // Copies a symlink now, schedules a subdirectory, or adds a regular file
    // to `files` for batching.
    void copy_entry(const std::shared_ptr<Directory>& directory, DirectoryEntry entry,
                    std::vector<std::string>& files) {
        const fs::path relative = directory->relative / entry.name;
        if (options_.skip && options_.skip(relative)) {
            return;
        }
        switch (entry.type) {
        case DT_DIR:
            directory->pending.fetch_add(1, std::memory_order_relaxed);
            queue_.push(
                [this, directory, name = std::move(entry.name)] { enter(directory, name); });
            break;
        case DT_REG:
            files.push_back(std::move(entry.name));
            break;
        case DT_LNK:
            copy_symlink(*directory, entry.name.c_str(), relative);
            break;
        default:
            throw unsupported(from_ / relative);
        }
    }

    // Replays `directory`'s metadata once it and everything below it are
    // copied, then does the same for each ancestor this completes.
    void finish(std::shared_ptr<Directory> directory) {
        detail::count_down(std::move(directory), [this](const Directory& finished) {
            replay_metadata(finished.source.get(), finished.destination.get(), finished.info,
                            finished.created, from_ / finished.relative, to_ / finished.relative);
        });
    }

    // The copy of one multiply linked source file. Its mutex is held while
    // the first link is copied, so later links wait only for that inode.
    struct LinkedCopy {
        std::mutex copying;
        fs::path path; // empty if the first copy failed
    };

    // For a file with several links: links `name` to the copy of an earlier
    // link and returns linked, or returns the held lock under which the caller
    // makes the first copy and records it with remember_link.
    struct LinkState {
        bool linked = false;
        std::shared_ptr<LinkedCopy> copy;
        std::unique_lock<std::mutex> first_copy;
    };
    LinkState link_existing(const Directory& directory, const char* name, const struct stat& info,
                            const fs::path& relative) {
        if (info.st_nlink <= 1) {
            return {};
        }
        std::shared_ptr<LinkedCopy> copy;
        {
            const std::scoped_lock lock(links_mutex_);
            auto [entry, first] = links_.try_emplace({info.st_dev, info.st_ino});
            if (first) {
                entry->second = std::make_shared<LinkedCopy>();
                // Locked before anyone else can see the entry.
                return {.linked = false,
                        .copy = entry->second,
                        .first_copy = std::unique_lock(entry->second->copying)};
            }
            copy = entry->second;
        }
        const std::scoped_lock wait(copy->copying);
        if (copy->path.empty()) {
            throw Error(ErrorKind::io, std::format("cannot link {}: its first link failed to copy",
                                                   (to_ / relative).string()));
        }
        if (::linkat(AT_FDCWD, copy->path.c_str(), directory.destination.get(), name, 0) != 0) {
            throw errno_error("link", to_ / relative);
        }
        return {.linked = true, .copy = {}, .first_copy = {}};
    }

    static void remember_link(LinkState& link, const fs::path& to) {
        if (link.first_copy.owns_lock()) {
            link.copy->path = to;
            link.first_copy.unlock();
        }
    }

    void copy_file(const Directory& directory, const char* name, const fs::path& relative) {
        const fs::path from = from_ / relative;
        const fs::path to = to_ / relative;
#ifdef __APPLE__
        if (options_.mode == CopyMode::cow) {
            clone_file_at(directory, name, relative);
            return;
        }
#endif
        // O_NONBLOCK: a fifo swapped in after listing must fail, not hang.
        const auto source =
            detail::Fd::open_at(directory.source.get(), name,
                                O_RDONLY | O_NOFOLLOW | O_NONBLOCK | O_NOCTTY, 0, "open", from);
        const struct stat info = stat_fd(source.get(), from);
        if (!S_ISREG(info.st_mode)) {
            throw unsupported(from);
        }
        auto link = link_existing(directory, name, info, relative);
        if (link.linked) {
            return;
        }
        // Clones ignore it, but byte copies must not see EAGAIN.
        if (options_.mode == CopyMode::copy && ::fcntl(source.get(), F_SETFL, 0) != 0) {
            throw errno_error("open", from);
        }
        const auto destination = detail::Fd::open_at(
            directory.destination.get(), name, O_WRONLY | O_CREAT | O_EXCL | O_NOFOLLOW,
            static_cast<mode_t>((info.st_mode & 0777) | S_IWUSR), "create", to);
        copy_contents(source.get(), destination.get(), options_.mode, from, to);
        remember_link(link, to);
        replay_metadata(source.get(), destination.get(), info,
                        created_as(stat_fd(destination.get(), to)), from, to);
    }

#ifdef __APPLE__
    // clonefile carries mode, xattrs, ACL, and timestamps. Only ownership, as
    // far as the caller may set it, and the set-ID bits clonefile clears need
    // replaying.
    void clone_file_at(const Directory& directory, const char* name, const fs::path& relative) {
        const fs::path from = from_ / relative;
        const fs::path to = to_ / relative;
        struct stat info{};
        if (::fstatat(directory.source.get(), name, &info, AT_SYMLINK_NOFOLLOW) != 0) {
            throw errno_error("stat", from);
        }
        if (!S_ISREG(info.st_mode)) {
            throw unsupported(from);
        }
        auto link = link_existing(directory, name, info, relative);
        if (link.linked) {
            return;
        }
        clone_at(directory.source.get(), name, directory.destination.get(), from);
        remember_link(link, to);
        // Without privilege a clone is owned as a new file would be: by the
        // caller, with the destination directory's group.
        const bool privileged = ::geteuid() == 0;
        const uid_t uid = privileged ? info.st_uid : ::geteuid();
        const gid_t gid = privileged ? info.st_gid : directory.created.gid;
        bool chowned = false;
        if (uid != info.st_uid || gid != info.st_gid) {
            if (::fchownat(directory.destination.get(), name, info.st_uid, info.st_gid,
                           AT_SYMLINK_NOFOLLOW) == 0) {
                chowned = true;
            } else if (errno != EPERM) {
                throw errno_error("set owner", to);
            }
        }
        if ((chowned || (info.st_mode & (S_ISUID | S_ISGID)) != 0) &&
            ::fchmodat(directory.destination.get(), name,
                       static_cast<mode_t>(info.st_mode & permission_bits),
                       AT_SYMLINK_NOFOLLOW) != 0) {
            throw errno_error("set permissions", to);
        }
    }
#endif

    void copy_symlink(const Directory& directory, const char* name, const fs::path& relative) {
        const fs::path from = from_ / relative;
        const fs::path to = to_ / relative;
        struct stat info{};
        if (::fstatat(directory.source.get(), name, &info, AT_SYMLINK_NOFOLLOW) != 0) {
            throw errno_error("stat", from);
        }
        std::string target(static_cast<size_t>(info.st_size) + 1, '\0');
        for (;;) {
            const ssize_t length =
                ::readlinkat(directory.source.get(), name, target.data(), target.size());
            if (length < 0) {
                throw errno_error("read link", from);
            }
            if (static_cast<size_t>(length) < target.size()) {
                target.resize(static_cast<size_t>(length));
                break;
            }
            target.resize(target.size() * 2); // the link changed since stat
        }
        if (::symlinkat(target.c_str(), directory.destination.get(), name) != 0) {
            throw errno_error("create symlink", to);
        }
        replay_symlink_metadata(from, to, info);
    }

    fs::path from_;
    fs::path to_;
    const CopyTreeOptions& options_;
    detail::WorkQueue queue_;
    std::mutex links_mutex_;
    std::map<std::pair<dev_t, ino_t>, std::shared_ptr<LinkedCopy>> links_;
};

} // namespace

void copy_tree(const fs::path& from, const fs::path& to, const CopyTreeOptions& options) {
    if (exists_nofollow(to)) {
        throw Error(ErrorKind::invalid_path,
                    std::format("destination already exists: {}", to.string()));
    }
    auto source_fd = detail::Fd::open(from, O_RDONLY | O_DIRECTORY, 0, "open directory");
    if (::mkdir(to.c_str(), private_mode) != 0) {
        throw errno_error("create directory", to);
    }
    try {
        detail::Fd destination = open_directory_as_owner(AT_FDCWD, to.c_str(), to);
        if (::fchmod(destination.get(), private_mode) != 0) {
            throw errno_error("set permissions", to);
        }
        TreeCopier(from, to, options).run(std::move(source_fd), std::move(destination));
    } catch (...) {
        try {
            remove_tree(to);
        } catch (...) { // NOLINT(bugprone-empty-catch): report the copy's failure
        }
        throw;
    }
}

} // namespace hz
