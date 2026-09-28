#include "hz/tree.hpp"

#include "hz/detail/fd.hpp"
#include "hz/detail/work_queue.hpp"
#include "hz/entry_facts.hpp"
#include "hz/error.hpp"
#include "hz/fsutil.hpp"
#include "hz/metadata.hpp"

#ifdef __APPLE__
#include <copyfile.h>
#endif

#include <algorithm>
#include <array>
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
#include <thread>
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
    // Whether the subtree clone check already covered this directory, so its
    // subdirectories are looked up instead of scanned again.
    bool scanned = false;
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
#ifdef __APPLE__
        if (options_.mode == CopyMode::cow) {
            if (clones_as_walked(*parent, *directory)) {
                clone_at(parent->source.get(), name.c_str(), parent->destination.get(), from);
                restore_directory_times(*parent, name, directory->relative);
                finish(parent);
                return;
            }
        }
#endif
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
    // Cloning a directory clones each entry below it as clone_file_at's
    // clonefileat would, in one call that APFS completes several times
    // faster than cloning the entries one by one, except that it drops ACLs
    // below the top and stamps every directory with the current time. Walking
    // also does more for skipped entries, special files, hard links, set-ID
    // files, and ownership the clone would not keep. A subtree without any of
    // these is cloned whole and its directories' times restored. Like the
    // walk, this assumes the source stays quiescent.

    // Whether `directory`, a subdirectory of `parent` opened but not yet
    // created, may be cloned whole. The first check of a subtree scans all of
    // it and records every directory below, so later checks within it are
    // lookups. Marks `directory` scanned.
    bool clones_as_walked(const Directory& parent, Directory& directory) {
        directory.scanned = true;
        if (parent.scanned) {
            const std::scoped_lock lock(scanned_mutex_);
            const auto found = scanned_.find(directory.relative.native());
            if (found != scanned_.end()) {
                // The scan expected clones in `parent` to get its source's
                // group, which its copy only has once replayed.
                return found->second.cloneable && parent.created.gid == parent.info.st_gid;
            }
        }
        const auto facts = entry_facts(directory.source.get(), from_ / directory.relative);
        return scan(directory.source.get(), directory.relative, facts, parent.created.gid);
    }

    // Whether an entry clones as walking would copy it into a directory
    // whose new entries get group `gid`.
    static bool clones_as_walked(const EntryFacts& facts, gid_t gid) {
        if (facts.uid != ::geteuid() || facts.gid != gid || facts.acl) {
            return false;
        }
        switch (facts.type) {
        case DT_REG:
            return facts.links == 1 && (facts.mode & (S_ISUID | S_ISGID)) == 0;
        case DT_DIR:
            return (facts.mode & S_IXUSR) != 0; // its times are restored through it
        case DT_LNK:
            return true;
        default:
            return false;
        }
    }

    // Whether the open directory `fd` at `relative`, described by `facts`
    // and cloned into a directory giving it group `gid`, clones as walked
    // with everything below it. Records it and each directory below.
    bool scan(int fd, const fs::path& relative, const EntryFacts& facts, gid_t gid) {
        bool cloneable = clones_as_walked(facts, gid);
        for (const auto& entry : list_entry_facts(fd, from_ / relative, true)) {
            const fs::path child = relative / entry.name;
            if (options_.skip && options_.skip(child)) {
                cloneable = false;
                continue;
            }
            if (entry.type == DT_DIR) {
                const auto subdirectory = open_directory_at(fd, entry.name.c_str(), from_ / child);
                // Entries below get this directory's group when cloned.
                if (!scan(subdirectory.get(), child, entry, facts.gid)) {
                    cloneable = false;
                }
            } else if (!clones_as_walked(entry, facts.gid)) {
                cloneable = false;
            }
        }
        const std::scoped_lock lock(scanned_mutex_);
        scanned_.insert_or_assign(relative.native(),
                                  Scanned{.cloneable = cloneable, .times = facts.times});
        return cloneable;
    }

    // Gives each directory of the subtree cloned as `name` in `parent`, from
    // `relative`, its source's access and modification times.
    void restore_directory_times(const Directory& parent, const std::string& name,
                                 const fs::path& relative) {
        std::vector<std::pair<std::string, std::array<timespec, 2>>> directories;
        {
            const std::scoped_lock lock(scanned_mutex_);
            directories.emplace_back(name, scanned_.at(relative.native()).times);
            // Every path below `relative` sorts together after this prefix.
            const std::string prefix = relative.native() + '/';
            for (auto it = scanned_.lower_bound(prefix);
                 it != scanned_.end() && it->first.starts_with(prefix); ++it) {
                directories.emplace_back(name + it->first.substr(relative.native().size()),
                                         it->second.times);
            }
        }
        for (const auto& [path, times] : directories) {
            if (::utimensat(parent.destination.get(), path.c_str(), times.data(),
                            AT_SYMLINK_NOFOLLOW) != 0) {
                throw errno_error("set times", to_ / parent.relative / path);
            }
        }
    }

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
#ifdef __APPLE__
    struct Scanned {
        bool cloneable;
        std::array<timespec, 2> times; // access, modification
    };
    std::mutex scanned_mutex_;
    std::map<std::string, Scanned> scanned_; // by relative directory path
#endif
};

#ifdef __APPLE__

// Clones the whole tree with one clonefile while the other workers scan the
// source, then fixes up where the clone differs from what walking it would
// produce. APFS clones a whole tree in about the time it takes to clone its
// largest subtrees one by one, so scanning first would only add to that.
class WholeTreeCloner {
  public:
    // Scanning competes with the clone for APFS; two threads finish well
    // within the clone's time.
    WholeTreeCloner(fs::path from, fs::path to, const CopyTreeOptions& options)
        : from_(std::move(from)), to_(std::move(to)), options_(options),
          queue_(std::min(2U, options.workers == 0 ? detail::default_workers() : options.workers)) {
    }

    // Returns false, leaving nothing at `to`, if the tree needs walking.
    bool run() {
        // Everything below the top takes the group of the clone's parent.
        struct stat parent{};
        if (::stat(to_.parent_path().c_str(), &parent) != 0) {
            throw errno_error("stat", to_.parent_path());
        }
        clone_gid_ = parent.st_gid;
        auto root = detail::Fd::open(from_, O_RDONLY | O_DIRECTORY, 0, "open directory");
        const auto facts = entry_facts(root.get(), from_);
        check_directory("", facts, /*top=*/true);
        if (blocked_) {
            return false;
        }
        queue_.push(
            [this, fd = std::make_shared<detail::Fd>(std::move(root))] { scan(fd->get(), ""); });

        std::exception_ptr clone_failure;
        std::thread clone([&] {
            try {
                clone_tree(from_, to_);
            } catch (...) {
                clone_failure = std::current_exception();
            }
        });
        std::exception_ptr scan_failure;
        try {
            queue_.run();
        } catch (...) {
            scan_failure = std::current_exception();
        }
        clone.join();
        if (clone_failure) {
            std::rethrow_exception(clone_failure);
        }
        try {
            if (scan_failure) {
                std::rethrow_exception(scan_failure);
            }
            if (blocked_) {
                remove_tree(to_);
                return false;
            }
            fix_up();
        } catch (...) {
            discard();
            throw;
        }
        return true;
    }

  private:
    // An entry whose ownership or set-ID bits the clone did not keep.
    struct Owned {
        std::string path;
        uid_t uid;
        gid_t gid;
        mode_t mode;
        bool symlink;
    };

    void discard() {
        try {
            remove_tree(to_);
        } catch (...) { // NOLINT(bugprone-empty-catch): report the original failure
        }
    }

    [[nodiscard]] fs::path source(const std::string& relative) const {
        return relative.empty() ? from_ : from_ / relative;
    }
    [[nodiscard]] fs::path copy(const std::string& relative) const {
        return relative.empty() ? to_ : to_ / relative;
    }

    // Records what the clone of the directory at `relative` needs; blocks
    // the whole-tree clone if fixing up below it needs owner access the
    // directory does not give.
    void check_directory(const std::string& relative, const EntryFacts& facts, bool top) {
        const std::scoped_lock lock(mutex_);
        if (facts.type != DT_DIR || (facts.mode & S_IRWXU) != S_IRWXU) {
            blocked_ = true;
            return;
        }
        directories_.emplace_back(relative, facts.times);
        record_ownership(relative, facts, top);
    }

    // Called with mutex_ held.
    void record_ownership(const std::string& relative, const EntryFacts& facts, bool top) {
        // The top keeps its ACL; entries below lose theirs.
        if (facts.acl && !top) {
            acls_.push_back(relative);
        }
        const bool owner = facts.uid != ::geteuid() || facts.gid != clone_gid_;
        const bool set_id = facts.type == DT_REG && (facts.mode & (S_ISUID | S_ISGID)) != 0;
        if (owner || set_id) {
            owned_.push_back({.path = relative,
                              .uid = facts.uid,
                              .gid = facts.gid,
                              .mode = facts.mode,
                              .symlink = facts.type == DT_LNK});
        }
    }

    void scan(int fd, const std::string& relative) {
        for (auto& entry : list_entry_facts(fd, source(relative), true)) {
            std::string child = relative.empty() ? entry.name : relative + '/' + entry.name;
            if (options_.skip && options_.skip(child)) {
                const std::scoped_lock lock(mutex_);
                if (entry.type == DT_DIR || entry.type == DT_UNKNOWN) {
                    blocked_ = true;
                } else {
                    removals_.push_back(std::move(child));
                }
                continue;
            }
            switch (entry.type) {
            case DT_DIR: {
                check_directory(child, entry, false);
                auto subdirectory = std::make_shared<detail::Fd>(
                    open_directory_at(fd, entry.name.c_str(), source(child)));
                queue_.push([this, subdirectory, child] { scan(subdirectory->get(), child); });
                break;
            }
            case DT_REG:
            case DT_LNK: {
                const std::scoped_lock lock(mutex_);
                if (entry.type == DT_REG && entry.links > 1) {
                    links_[{entry.device, entry.inode}].push_back(child);
                }
                record_ownership(child, entry, false);
                break;
            }
            case DT_UNKNOWN: {
                const std::scoped_lock lock(mutex_);
                blocked_ = true;
                break;
            }
            default:
                throw unsupported(source(child));
            }
        }
    }

    // Brings the clone to what walking would have produced. Structural
    // changes first, as they touch directory times; ACLs last, as a copied
    // ACL may deny the owner the other changes.
    void fix_up() {
        for (const auto& removal : removals_) {
            if (::unlink(copy(removal).c_str()) != 0) {
                throw errno_error("remove", copy(removal));
            }
        }
        for (const auto& [inode, paths] : links_) {
            // Each later link becomes a link to the first one's copy.
            for (std::size_t i = 1; i < paths.size(); ++i) {
                if (::unlink(copy(paths[i]).c_str()) != 0 ||
                    ::link(copy(paths[0]).c_str(), copy(paths[i]).c_str()) != 0) {
                    throw errno_error("link", copy(paths[i]));
                }
            }
        }
        for (const auto& entry : owned_) {
            restore_ownership(entry);
        }
        parallel(directories_.size(), [this](std::size_t i) {
            const auto& [relative, times] = directories_[i];
            if (::utimensat(AT_FDCWD, copy(relative).c_str(), times.data(), AT_SYMLINK_NOFOLLOW) !=
                0) {
                throw errno_error("set times", copy(relative));
            }
        });
        for (const auto& relative : acls_) {
            if (::copyfile(source(relative).c_str(), copy(relative).c_str(), nullptr,
                           COPYFILE_ACL | COPYFILE_NOFOLLOW) != 0) {
                throw errno_error("set ACL", copy(relative));
            }
        }
    }

    // As walking would: ownership as far as the caller may set it, then the
    // mode, which chown and the clone may have stripped of set-ID bits.
    void restore_ownership(const Owned& entry) const {
        const fs::path path = copy(entry.path);
        if (::lchown(path.c_str(), entry.uid, entry.gid) != 0 && errno != EPERM) {
            throw errno_error("set owner", path);
        }
        if (!entry.symlink && ::chmod(path.c_str(), entry.mode) != 0) {
            throw errno_error("set permissions", path);
        }
    }

    // Runs `work(i)` for every i below `count` on the workers.
    template <typename Work> void parallel(std::size_t count, Work work) {
        constexpr std::size_t chunk = 256;
        detail::WorkQueue queue(options_.workers == 0 ? detail::default_workers()
                                                      : options_.workers);
        for (std::size_t start = 0; start < count; start += chunk) {
            queue.push([start, count, &work] {
                for (std::size_t i = start; i < std::min(count, start + chunk); ++i) {
                    work(i);
                }
            });
        }
        queue.run();
    }

    fs::path from_;
    fs::path to_;
    const CopyTreeOptions& options_;
    detail::WorkQueue queue_;
    gid_t clone_gid_ = 0;
    std::mutex mutex_;
    bool blocked_ = false;
    std::vector<std::string> removals_;
    std::map<std::pair<dev_t, ino_t>, std::vector<std::string>> links_;
    std::vector<Owned> owned_;
    std::vector<std::string> acls_;
    std::vector<std::pair<std::string, std::array<timespec, 2>>> directories_;
};

#endif

} // namespace

void copy_tree(const fs::path& from, const fs::path& to, const CopyTreeOptions& options) {
    if (exists_nofollow(to)) {
        throw Error(ErrorKind::invalid_path,
                    std::format("destination already exists: {}", to.string()));
    }
#ifdef __APPLE__
    if (options.mode == CopyMode::cow && options.clone_whole_tree &&
        WholeTreeCloner(from, to, options).run()) {
        return;
    }
#endif
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
