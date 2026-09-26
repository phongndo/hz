#include "hz/fsutil.hpp"

#include "hz/error.hpp"
#include "hz/marker.hpp"

#include <algorithm>
#include <cerrno>
#include <csignal>
#include <cstdio>
#include <dirent.h>
#include <format>
#include <memory>
#include <string>
#include <sys/file.h>
#include <sys/stat.h>
#include <unistd.h>
#include <vector>

#ifdef __linux__
#include <fcntl.h>
#include <linux/fs.h>
#include <sys/syscall.h>
#endif

namespace hz {

namespace fs = std::filesystem;

detail::Fd lock_operations(const fs::path& data_directory) {
    make_private_directories(data_directory);
    auto lock = detail::Fd::open(data_directory / "operations.lock", O_RDWR | O_CREAT | O_NOFOLLOW,
                                 0600, "open operation lock");
    if (::flock(lock.get(), LOCK_EX | LOCK_NB) != 0) {
        if (errno == EWOULDBLOCK || errno == EAGAIN) {
            throw Error(ErrorKind::conflict,
                        "another hz operation is in progress; retry when it finishes "
                        "(hooks must not run mutating hz commands)");
        }
        throw errno_error("lock operations", data_directory);
    }
    return lock;
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

bool same_filesystem(const fs::path& a, const fs::path& b) {
    struct stat first{};
    struct stat second{};
    if (::stat(a.c_str(), &first) != 0) {
        throw errno_error("stat", a);
    }
    if (::stat(b.c_str(), &second) != 0) {
        throw errno_error("stat", b);
    }
    return first.st_dev == second.st_dev;
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

void remove_contents(const fs::path& directory, bool keep_marker) {
    // Make sure we can list and unlink entries even if the tree was read-only.
    if (::chmod(directory.c_str(), 0700) != 0) {
        throw errno_error("set directory permissions", directory);
    }
    std::unique_ptr<DIR, decltype(&closedir)> handle(::opendir(directory.c_str()), &closedir);
    if (!handle) {
        throw errno_error("open directory", directory);
    }
    std::vector<std::string> names;
    for (;;) {
        errno = 0;
        const dirent* entry = ::readdir(handle.get());
        if (entry == nullptr) {
            if (errno != 0) {
                throw errno_error("read directory", directory);
            }
            break;
        }
        std::string name = &entry->d_name[0];
        if (name != "." && name != ".." && !(keep_marker && name == marker_name)) {
            names.push_back(std::move(name));
        }
    }
    handle.reset();
    for (const auto& name : names) {
        const fs::path child = directory / name;
        struct stat info{};
        if (::lstat(child.c_str(), &info) != 0) {
            if (errno == ENOENT) {
                continue;
            }
            throw errno_error("stat", child);
        }
        if (S_ISDIR(info.st_mode)) {
            remove_contents(child, false);
            if (::rmdir(child.c_str()) != 0) {
                throw errno_error("remove directory", child);
            }
        } else if (::unlink(child.c_str()) != 0 && errno != ENOENT) {
            throw errno_error("remove", child);
        }
    }
}

} // namespace

void remove_tree(const fs::path& directory) {
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
    remove_contents(directory, true);
    remove_marker(directory);
    if (::rmdir(directory.c_str()) != 0) {
        throw errno_error("remove directory", directory);
    }
}

bool process_alive(std::int64_t pid) {
    if (pid <= 0) {
        return false;
    }
    return ::kill(static_cast<pid_t>(pid), 0) == 0 || errno == EPERM;
}

} // namespace hz
