#include "hz/fsutil.hpp"

#include "hz/detail/work_queue.hpp"
#include "hz/error.hpp"
#include "hz/marker.hpp"
#include "hz/ulid.hpp"

#include <algorithm>
#include <atomic>
#include <cerrno>
#include <chrono>
#include <csignal>
#include <cstdio>
#include <cstdlib>
#include <dirent.h>
#include <fcntl.h>
#include <format>
#include <memory>
#include <optional>
#include <string>
#include <sys/file.h>
#include <sys/stat.h>
#include <thread>
#include <unistd.h>
#include <utility>
#include <vector>

#ifdef __linux__
#include <array>
#include <cstdint>
#include <linux/btrfs.h>
#include <linux/fs.h>
#include <linux/magic.h>
#include <sys/ioctl.h>
#include <sys/syscall.h>
#include <sys/vfs.h>
#endif

namespace hz {

namespace fs = std::filesystem;

namespace {

fs::path lease_path(const fs::path& data_directory, std::string_view id) {
    return data_directory / "leases" / id;
}

// Whether `path` still names the file open as `fd`: a lease file can be
// unlinked by doctor while another process holds it open.
bool names_open_file(const fs::path& path, int fd) {
    struct stat held{};
    struct stat current{};
    if (::fstat(fd, &held) != 0) {
        throw errno_error("stat", path);
    }
    if (::lstat(path.c_str(), &current) != 0) {
        if (errno == ENOENT) {
            return false;
        }
        throw errno_error("stat", path);
    }
    return current.st_dev == held.st_dev && current.st_ino == held.st_ino;
}

} // namespace

namespace {

// Whether this process runs as a hook of an hz process that still holds its
// hook lease, and so may hold the lock. A leftover or unreadable marker is
// ignored.
bool hook_parent_holds_lock(const fs::path& data_directory) {
    const char* token = std::getenv(hook_parent_variable);
    if (token == nullptr || !is_ulid(token)) {
        return false;
    }
    try {
        return lease_state(data_directory, token) == Lease::held;
    } catch (const Error&) {
        return false;
    }
}

} // namespace

detail::Fd lock_operations(const fs::path& data_directory, std::chrono::seconds wait) {
    make_private_directories(data_directory);
    auto lock = detail::Fd::open(data_directory / "operations.lock", O_RDWR | O_CREAT | O_NOFOLLOW,
                                 0600, "open operation lock");
    // A hook's parent hz process may hold the lock, so waiting could only
    // time out. It holds the lease its hooks name while they run; processes
    // that outlive it, or use another registry, may wait.
    if (hook_parent_holds_lock(data_directory)) {
        throw Error(ErrorKind::conflict,
                    "hooks must not run mutating hz commands; run them after the outer command "
                    "finishes");
    }
    const auto deadline = std::chrono::steady_clock::now() + wait;
    auto delay = std::chrono::milliseconds(1);
    while (::flock(lock.get(), LOCK_EX | LOCK_NB) != 0) {
        if (errno == EINTR) {
            continue;
        }
        if (errno != EWOULDBLOCK && errno != EAGAIN) {
            throw errno_error("lock operations", data_directory);
        }
        if (std::chrono::steady_clock::now() >= deadline) {
            throw Error(ErrorKind::conflict,
                        std::format("another hz operation has been in progress for over {} "
                                    "seconds; retry when it finishes",
                                    wait.count()));
        }
        std::this_thread::sleep_for(delay);
        delay = std::min(delay * 2, std::chrono::milliseconds(20));
    }
    return lock;
}

detail::Fd acquire_lease(const fs::path& data_directory, std::string_view id, bool wait) {
    const fs::path path = lease_path(data_directory, id);
    make_private_directories(path.parent_path());
    for (;;) {
        auto lease = detail::Fd::open(path, O_RDWR | O_CREAT | O_NOFOLLOW, 0600, "open lease");
        int status = 0;
        while ((status = ::flock(lease.get(), wait ? LOCK_EX : LOCK_EX | LOCK_NB)) != 0 &&
               errno == EINTR) {
        }
        if (status != 0) {
            if (errno == EWOULDBLOCK || errno == EAGAIN) {
                throw Error(ErrorKind::conflict,
                            std::format("workspace {} is busy in another hz process", id));
            }
            throw errno_error("lease", path);
        }
        // Doctor may have removed the unlocked file between open and flock;
        // a lease on an unlinked file protects nothing.
        if (names_open_file(path, lease.get())) {
            return lease;
        }
    }
}

Lease lease_state(const fs::path& data_directory, std::string_view id) {
    const fs::path path = lease_path(data_directory, id);
    const int fd = ::open(path.c_str(), O_RDONLY | O_NOFOLLOW | O_CLOEXEC);
    if (fd < 0) {
        if (errno == ENOENT) {
            return Lease::absent;
        }
        throw errno_error("open lease", path);
    }
    const detail::Fd lease(fd);
    if (::flock(fd, LOCK_SH | LOCK_NB) == 0) {
        return Lease::free;
    }
    if (errno == EWOULDBLOCK || errno == EAGAIN) {
        return Lease::held;
    }
    throw errno_error("check lease", path);
}

void remove_free_lease(const fs::path& data_directory, std::string_view id) {
    const fs::path path = lease_path(data_directory, id);
    const int fd = ::open(path.c_str(), O_RDONLY | O_NOFOLLOW | O_CLOEXEC);
    if (fd < 0) {
        return;
    }
    const detail::Fd lease(fd);
    // Holding it exclusively while unlinking makes a concurrent acquirer wait,
    // then notice the file is gone and start a new one.
    if (::flock(fd, LOCK_EX | LOCK_NB) == 0 && names_open_file(path, fd)) {
        ::unlink(path.c_str());
    }
}

void release_lease(const fs::path& data_directory, std::string_view id,
                   [[maybe_unused]] detail::Fd lease) {
    const fs::path path = lease_path(data_directory, id);
    if (::unlink(path.c_str()) != 0 && errno != ENOENT) {
        throw errno_error("remove lease", path);
    }
    // `lease` closes after the unlink, so nobody can lock the old file anew.
}

void make_private_directories(const fs::path& directory) {
    if (exists_nofollow(directory)) {
        return;
    }
    if (directory.has_parent_path() && directory.parent_path() != directory) {
        make_private_directories(directory.parent_path());
    }
    if (::mkdir(directory.c_str(), 0700) != 0 && errno != EEXIST) {
        throw errno_error("create directory", directory);
    }
}

bool exists_nofollow(const fs::path& path) {
    struct stat info{};
    if (::lstat(path.c_str(), &info) == 0) {
        return true;
    }
    if (errno == ENOENT || errno == ENOTDIR) {
        return false;
    }
    throw errno_error("stat", path);
}

#ifdef __linux__
namespace {

// The UUID of the btrfs filesystem holding `path`, which every subvolume of
// it shares although each has its own device number.
std::optional<std::array<std::uint8_t, BTRFS_FSID_SIZE>> btrfs_filesystem(const fs::path& path) {
    const int fd = ::open(path.c_str(), O_RDONLY | O_DIRECTORY | O_CLOEXEC);
    if (fd < 0) {
        return std::nullopt;
    }
    const detail::Fd directory(fd);
    struct statfs filesystem{};
    btrfs_ioctl_fs_info_args info{};
    if (::fstatfs(fd, &filesystem) != 0 ||
        std::cmp_not_equal(filesystem.f_type, BTRFS_SUPER_MAGIC) ||
        ::ioctl(fd, BTRFS_IOC_FS_INFO, &info) != 0) {
        return std::nullopt;
    }
    std::array<std::uint8_t, BTRFS_FSID_SIZE> fsid{};
    std::ranges::copy(info.fsid, fsid.begin());
    return fsid;
}

} // namespace
#endif

bool same_filesystem(const fs::path& a, const fs::path& b) {
    struct stat first{};
    struct stat second{};
    if (::stat(a.c_str(), &first) != 0) {
        throw errno_error("stat", a);
    }
    if (::stat(b.c_str(), &second) != 0) {
        throw errno_error("stat", b);
    }
    if (first.st_dev == second.st_dev) {
        return true;
    }
#ifdef __linux__
    const auto filesystem = btrfs_filesystem(a);
    return filesystem && filesystem == btrfs_filesystem(b);
#else
    return false;
#endif
}

bool is_within(const fs::path& candidate, const fs::path& ancestor) {
    auto [end, _] = std::ranges::mismatch(ancestor, candidate);
    return end == ancestor.end();
}

void move_path(const fs::path& from, const fs::path& to) {
#ifdef __linux__
    // RENAME_NOREPLACE makes the no-clobber check atomic.
    if (::syscall(SYS_renameat2, AT_FDCWD, from.c_str(), AT_FDCWD, to.c_str(), RENAME_NOREPLACE) ==
        0) {
        return;
    }
#elifdef __APPLE__
    if (::renamex_np(from.c_str(), to.c_str(), RENAME_EXCL) == 0) {
        return;
    }
#endif
    // A check-then-rename fallback can overwrite a destination another process
    // creates between the check and rename. Require atomic no-replace support.
    throw errno_error(std::format("move to {} without replacing it", to.string()), from);
}

namespace {

// The DT_* type of `name`, for filesystems whose listings omit it; nothing
// if the entry has vanished.
std::optional<unsigned char> type_of(int directory, const std::string& name, const fs::path& path) {
    struct stat info{};
    if (::fstatat(directory, name.c_str(), &info, AT_SYMLINK_NOFOLLOW) != 0) {
        if (errno == ENOENT) {
            return std::nullopt;
        }
        throw errno_error("stat", path / name);
    }
    return S_ISDIR(info.st_mode)   ? DT_DIR
           : S_ISREG(info.st_mode) ? DT_REG
           : S_ISLNK(info.st_mode) ? DT_LNK
                                   : DT_FIFO;
}

} // namespace

std::vector<DirectoryEntry> read_directory(int directory, const fs::path& path) {
    const int handle = ::fcntl(directory, F_DUPFD_CLOEXEC, 0);
    if (handle < 0) {
        throw errno_error("open directory", path);
    }
    const std::unique_ptr<DIR, decltype(&::closedir)> stream(::fdopendir(handle), &::closedir);
    if (!stream) {
        ::close(handle);
        throw errno_error("open directory", path);
    }
    // The duplicate shares the caller's position; list from the start.
    ::rewinddir(stream.get());
    std::vector<DirectoryEntry> entries;
    for (;;) {
        errno = 0;
        const dirent* entry = ::readdir(stream.get());
        if (entry == nullptr) {
            if (errno != 0) {
                throw errno_error("read directory", path);
            }
            break;
        }
        std::string name = &entry->d_name[0];
        if (name == "." || name == "..") {
            continue;
        }
        const auto type = entry->d_type == DT_UNKNOWN ? type_of(directory, name, path)
                                                      : std::optional(entry->d_type);
        if (!type) {
            continue;
        }
        const bool special = *type != DT_DIR && *type != DT_REG && *type != DT_LNK;
        entries.push_back({.name = std::move(name),
                           .type = special ? static_cast<unsigned char>(DT_FIFO) : *type});
    }
    return entries;
}

namespace {

// Makes the directory `name` in `parent` accessible to its owner, never
// following a symlink swapped in for it. Returns false with errno set.
bool chmod_directory_at(int parent, const char* name) {
    if (::fchmodat(parent, name, 0700, AT_SYMLINK_NOFOLLOW) == 0) {
        return true;
    }
    if (errno != ENOTSUP && errno != EOPNOTSUPP) {
        return false;
    }
    // Older C libraries cannot chmod without following; check it is still a
    // directory first.
    struct stat info{};
    if (::fstatat(parent, name, &info, AT_SYMLINK_NOFOLLOW) != 0) {
        return false;
    }
    if (!S_ISDIR(info.st_mode)) {
        errno = ENOTDIR;
        return false;
    }
    return ::fchmodat(parent, name, 0700, 0) == 0;
}

} // namespace

detail::Fd open_directory_as_owner(int parent, const char* name, const fs::path& path) {
    constexpr int flags = O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC;
    int fd = ::openat(parent, name, flags);
    if (fd < 0 && errno == EACCES) {
        if (!chmod_directory_at(parent, name)) {
            throw errno_error("set directory permissions", path);
        }
        fd = ::openat(parent, name, flags);
    }
    if (fd < 0) {
        throw errno_error("open directory", path);
    }
    return detail::Fd(fd);
}

namespace {

// Opens the directory `name` in `parent` for emptying, first making it
// accessible if the tree was read-only.
detail::Fd open_for_removal(int parent, const char* name, const fs::path& path) {
    auto directory = open_directory_as_owner(parent, name, path);
    const int fd = directory.get();
    struct stat info{};
    if (::fstat(fd, &info) != 0) {
        throw errno_error("stat", path);
    }
    // Unlinking entries needs write and search permission.
    if ((info.st_mode & 0700) != 0700 && ::fchmod(fd, 0700) != 0) {
        throw errno_error("set directory permissions", path);
    }
    return directory;
}

// A directory being emptied; it is removed from its parent once everything
// below it is gone.
struct Removal {
    detail::Fd fd{-1};
    fs::path path;
    std::string name;
    std::shared_ptr<Removal> parent;
    // Its own listing plus each subdirectory and file batch not yet removed.
    std::atomic<std::size_t> pending{1};
};

class TreeRemover {
  public:
    explicit TreeRemover(unsigned workers)
        : queue_(workers == 0 ? detail::default_workers() : workers) {}

    // Empties `directory`, leaving its workspace marker.
    void run(const fs::path& directory) {
        auto root = std::make_shared<Removal>();
        root->path = directory;
        root->fd = open_for_removal(AT_FDCWD, directory.c_str(), directory);
        queue_.push([this, root] { empty(root, true); });
        queue_.run();
    }

  private:
    void enter(const std::shared_ptr<Removal>& parent, const std::string& name) {
        auto directory = std::make_shared<Removal>();
        directory->path = parent->path / name;
        directory->name = name;
        directory->parent = parent;
        try {
            directory->fd = open_for_removal(parent->fd.get(), name.c_str(), directory->path);
        } catch (const Error& error) {
            if (error.code() != std::errc::no_such_file_or_directory) {
                throw;
            }
            finish(parent);
            return;
        }
        empty(directory, false);
    }

    void empty(const std::shared_ptr<Removal>& directory, bool keep_marker) {
        // Listing completes before anything is unlinked: some filesystems,
        // including APFS, skip entries of a directory changed mid-listing.
        std::vector<std::string> files;
        for (auto& entry : read_directory(directory->fd.get(), directory->path)) {
            if (keep_marker && entry.name == marker_name) {
                continue;
            }
            if (entry.type == DT_DIR) {
                directory->pending.fetch_add(1, std::memory_order_relaxed);
                queue_.push(
                    [this, directory, name = std::move(entry.name)] { enter(directory, name); });
                continue;
            }
            files.push_back(std::move(entry.name));
            if (files.size() == detail::file_batch) {
                directory->pending.fetch_add(1, std::memory_order_relaxed);
                queue_.push(
                    [directory, batch = std::move(files)] { unlink_files(directory, batch); });
                files.clear();
            }
        }
        unlink_files(directory, files);
    }

    // Unlinks non-directories of `directory`, then counts the batch finished.
    static void unlink_files(const std::shared_ptr<Removal>& directory,
                             const std::vector<std::string>& names) {
        for (const auto& name : names) {
            if (::unlinkat(directory->fd.get(), name.c_str(), 0) != 0 && errno != ENOENT) {
                throw errno_error("remove", directory->path / name);
            }
        }
        finish(directory);
    }

    static void finish(std::shared_ptr<Removal> directory) {
        detail::count_down(std::move(directory), [](const Removal& finished) {
            if (finished.parent &&
                ::unlinkat(finished.parent->fd.get(), finished.name.c_str(), AT_REMOVEDIR) != 0 &&
                errno != ENOENT) {
                throw errno_error("remove directory", finished.path);
            }
        });
    }

    detail::WorkQueue queue_;
};

} // namespace

void remove_tree(const fs::path& directory, unsigned workers) {
    struct stat info{};
    if (::lstat(directory.c_str(), &info) != 0) {
        if (errno == ENOENT) {
            return;
        }
        throw errno_error("stat", directory);
    }
    if (!S_ISDIR(info.st_mode)) {
        throw Error(ErrorKind::invalid_path,
                    std::format("{} is not a directory", directory.string()));
    }
    TreeRemover(workers).run(directory);
    remove_marker(directory);
    if (::rmdir(directory.c_str()) != 0) {
        throw errno_error("remove directory", directory);
    }
}

WorkerSlots::WorkerSlots(const fs::path& data_directory) {
    const unsigned slots = detail::default_workers();
    if (slots <= 1) {
        return;
    }
    const fs::path directory = data_directory / "workers";
    make_private_directories(directory);
    // Processes starting together try the slots from different places, so
    // each is likely to get some.
    const auto start = static_cast<unsigned>(::getpid());
    for (unsigned i = 0; i < slots; ++i) {
        const fs::path path = directory / std::to_string((start + i) % slots);
        auto slot = detail::Fd::open(path, O_RDWR | O_CREAT | O_NOFOLLOW, 0600, "open worker slot");
        int status = 0;
        while ((status = ::flock(slot.get(), LOCK_EX | LOCK_NB)) != 0 && errno == EINTR) {
        }
        if (status == 0) {
            held_.push_back(std::move(slot));
        } else if (errno != EWOULDBLOCK && errno != EAGAIN) {
            throw errno_error("take worker slot", path);
        }
    }
}

unsigned WorkerSlots::count() const noexcept {
    return std::max(1U, static_cast<unsigned>(held_.size()));
}

bool process_alive(std::int64_t pid) {
    if (pid <= 0) {
        return false;
    }
    return ::kill(static_cast<pid_t>(pid), 0) == 0 || errno == EPERM;
}

} // namespace hz
