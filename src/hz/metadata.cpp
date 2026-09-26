#include "hz/metadata.hpp"

#include "hz/error.hpp"

#include <algorithm>
#include <array>
#include <cerrno>
#include <cstring>
#include <fcntl.h>
#include <string>
#include <sys/stat.h>
#include <sys/xattr.h>
#include <unistd.h>
#include <vector>

#ifdef __APPLE__
#include <copyfile.h>
#endif

namespace hz {

namespace {

constexpr mode_t permission_bits = 07777;

#ifdef __linux__
ssize_t list_xattrs(const char* path, char* names, size_t size) {
    return ::llistxattr(path, names, size);
}
ssize_t get_xattr(const char* path, const char* name, char* value, size_t size) {
    return ::lgetxattr(path, name, value, size);
}
int set_xattr(const char* path, const char* name, const char* value, size_t size) {
    return ::lsetxattr(path, name, value, size, 0);
}
#elifdef __APPLE__
ssize_t list_xattrs(const char* path, char* names, size_t size) {
    return ::listxattr(path, names, size, XATTR_NOFOLLOW);
}
ssize_t get_xattr(const char* path, const char* name, char* value, size_t size) {
    return ::getxattr(path, name, value, size, 0, XATTR_NOFOLLOW);
}
int set_xattr(const char* path, const char* name, const char* value, size_t size) {
    return ::setxattr(path, name, value, size, 0, XATTR_NOFOLLOW);
}
#endif

void copy_xattrs(const std::filesystem::path& from, const std::filesystem::path& to) {
    ssize_t size = list_xattrs(from.c_str(), nullptr, 0);
    if (size < 0) {
        if (errno == ENOTSUP) {
            return;
        }
        throw errno_error("list attributes", from);
    }
    if (size == 0) {
        return;
    }
    std::vector<char> names(static_cast<size_t>(size));
    size = list_xattrs(from.c_str(), names.data(), names.size());
    if (size < 0) {
        throw errno_error("list attributes", from);
    }
    names.resize(static_cast<size_t>(size));

    std::vector<std::string> keys;
    for (size_t offset = 0; offset < names.size();) {
        keys.emplace_back(names.data() + offset);
        offset += keys.back().size() + 1;
    }
    // An access ACL may remove our temporary write permission. Apply it only
    // after the user attributes that need that permission have been copied.
    std::ranges::stable_partition(
        keys, [](const std::string& key) { return key != "system.posix_acl_access"; });
    std::vector<char> value;
    for (const auto& key : keys) {
        const char* name = key.c_str();
        ssize_t length = get_xattr(from.c_str(), name, nullptr, 0);
        if (length < 0) {
            throw errno_error("read attribute", from);
        }
        value.resize(static_cast<size_t>(length));
        if (length > 0) {
            length = get_xattr(from.c_str(), name, value.data(), value.size());
            if (length < 0) {
                throw errno_error("read attribute", from);
            }
        }
        if (set_xattr(to.c_str(), name, value.data(), static_cast<size_t>(length)) != 0) {
            // Privileged namespaces (Linux security.*, trusted.*) and
            // attributes the destination cannot hold are not the copy's
            // concern; everything the caller owns is preserved.
            if (errno == EPERM || errno == ENOTSUP || errno == EACCES) {
                continue;
            }
            throw errno_error("write attribute", to);
        }
    }
}

} // namespace

void replay_metadata(const std::filesystem::path& from, const std::filesystem::path& to,
                     EntryKind kind) {
    struct stat info{};
    if (::lstat(from.c_str(), &info) != 0) {
        throw errno_error("stat", from);
    }
    // A caller that can read an entry may still be unable to assume its
    // owner or group; keep the caller's ownership in that case.
    if (::lchown(to.c_str(), info.st_uid, info.st_gid) != 0 && errno != EPERM) {
        throw errno_error("set owner", to);
    }
    if (kind != EntryKind::symlink &&
        ::chmod(to.c_str(), (info.st_mode & permission_bits) | S_IWUSR) != 0) {
        throw errno_error("set permissions", to);
    }
    copy_xattrs(from, to);
    if (kind != EntryKind::symlink && ::chmod(to.c_str(), info.st_mode & permission_bits) != 0) {
        throw errno_error("set permissions", to);
    }

#ifdef __APPLE__
    const std::array<timespec, 2> times{info.st_atimespec, info.st_mtimespec};
#else
    const std::array<timespec, 2> times{info.st_atim, info.st_mtim};
#endif
    if (::utimensat(AT_FDCWD, to.c_str(), times.data(), AT_SYMLINK_NOFOLLOW) != 0) {
        throw errno_error("set times", to);
    }
#ifdef __APPLE__
    // Darwin ACLs are not xattrs. Directories and byte copies never go through
    // clonefile(CLONE_ACL), so replay their ACL explicitly, after other metadata
    // writes that the source ACL might prohibit.
    if (kind != EntryKind::symlink &&
        ::copyfile(from.c_str(), to.c_str(), nullptr,
                   COPYFILE_ACL | COPYFILE_NOFOLLOW_SRC | COPYFILE_NOFOLLOW_DST) != 0) {
        throw errno_error("set ACL", to);
    }
#endif
}

} // namespace hz
