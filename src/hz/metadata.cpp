#include "hz/metadata.hpp"

#include "hz/error.hpp"

#include <algorithm>
#include <array>
#include <cerrno>
#include <fcntl.h>
#include <string>
#include <sys/xattr.h>
#include <unistd.h>
#include <vector>

#ifdef __APPLE__
#include <copyfile.h>
#endif

namespace hz {

namespace {

// Extended-attribute calls on either an open descriptor or a symlink path.
struct FdAttributes {
    int fd;
#ifdef __linux__
    ssize_t list(char* names, size_t size) const { return ::flistxattr(fd, names, size); }
    ssize_t get(const char* name, char* value, size_t size) const {
        return ::fgetxattr(fd, name, value, size);
    }
    int set(const char* name, const char* value, size_t size) const {
        return ::fsetxattr(fd, name, value, size, 0);
    }
#elifdef __APPLE__
    ssize_t list(char* names, size_t size) const { return ::flistxattr(fd, names, size, 0); }
    ssize_t get(const char* name, char* value, size_t size) const {
        return ::fgetxattr(fd, name, value, size, 0, 0);
    }
    int set(const char* name, const char* value, size_t size) const {
        return ::fsetxattr(fd, name, value, size, 0, 0);
    }
#endif
};

struct SymlinkAttributes {
    const char* path;
#ifdef __linux__
    ssize_t list(char* names, size_t size) const { return ::llistxattr(path, names, size); }
    ssize_t get(const char* name, char* value, size_t size) const {
        return ::lgetxattr(path, name, value, size);
    }
    int set(const char* name, const char* value, size_t size) const {
        return ::lsetxattr(path, name, value, size, 0);
    }
#elifdef __APPLE__
    ssize_t list(char* names, size_t size) const {
        return ::listxattr(path, names, size, XATTR_NOFOLLOW);
    }
    ssize_t get(const char* name, char* value, size_t size) const {
        return ::getxattr(path, name, value, size, 0, XATTR_NOFOLLOW);
    }
    int set(const char* name, const char* value, size_t size) const {
        return ::setxattr(path, name, value, size, 0, XATTR_NOFOLLOW);
    }
#endif
};

// Copies every attribute the caller may set; returns whether an access ACL,
// which rewrites the mode, was written. The destination must be writable by
// its owner while this runs.
template <typename Attributes>
bool copy_xattrs(const Attributes& from, const Attributes& to, const std::filesystem::path& source,
                 const std::filesystem::path& destination) {
    // Most entries have no attributes: one call answers that.
    std::array<char, 1024> small{};
    std::vector<char> names;
    ssize_t size = from.list(small.data(), small.size());
    if (size < 0 && errno == ERANGE) {
        size = from.list(nullptr, 0);
        if (size > 0) {
            names.resize(static_cast<size_t>(size));
            size = from.list(names.data(), names.size());
        }
    } else if (size > 0) {
        names.assign(small.data(), small.data() + size);
    }
    if (size < 0) {
        if (errno == ENOTSUP) {
            return false;
        }
        throw errno_error("list attributes", source);
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
    bool acl = false;
    std::vector<char> value;
    for (const auto& key : keys) {
        const char* name = key.c_str();
        ssize_t length = from.get(name, nullptr, 0);
        if (length < 0) {
            throw errno_error("read attribute", source);
        }
        value.resize(static_cast<size_t>(length));
        if (length > 0) {
            length = from.get(name, value.data(), value.size());
            if (length < 0) {
                throw errno_error("read attribute", source);
            }
        }
        if (to.set(name, value.data(), static_cast<size_t>(length)) != 0) {
            // Privileged namespaces (Linux security.*, trusted.*) and
            // attributes the destination cannot hold are not the copy's
            // concern; everything the caller owns is preserved.
            if (errno == EPERM || errno == ENOTSUP || errno == EACCES) {
                continue;
            }
            throw errno_error("write attribute", destination);
        }
        acl = acl || key == "system.posix_acl_access";
    }
    return acl;
}

std::array<timespec, 2> times_of(const struct stat& info) {
#ifdef __APPLE__
    return {info.st_atimespec, info.st_mtimespec};
#else
    return {info.st_atim, info.st_mtim};
#endif
}

} // namespace

void replay_metadata(int source, int destination, const struct stat& info, const CreatedAs& created,
                     const std::filesystem::path& from, const std::filesystem::path& to) {
    // A caller that can read an entry may still be unable to assume its
    // owner or group; keep the caller's ownership in that case.
    bool chowned = false;
    if (created.uid != info.st_uid || created.gid != info.st_gid) {
        if (::fchown(destination, info.st_uid, info.st_gid) == 0) {
            chowned = true;
        } else if (errno != EPERM) {
            throw errno_error("set owner", to);
        }
    }
    // Attributes need owner write, which a restrictive umask can withhold.
    const bool writable = (created.mode & S_IWUSR) != 0;
    if (!writable && ::fchmod(destination, static_cast<mode_t>(created.mode | S_IWUSR)) != 0) {
        throw errno_error("set permissions", to);
    }
    const bool acl = copy_xattrs(FdAttributes{source}, FdAttributes{destination}, from, to);
    // chown can clear set-ID bits and an ACL rewrites the mode, so reapply the
    // source's bits after either, and whenever the mode differs.
    const auto mode = static_cast<mode_t>(info.st_mode & permission_bits);
    if ((chowned || acl || !writable || created.mode != mode) && ::fchmod(destination, mode) != 0) {
        throw errno_error("set permissions", to);
    }
    const auto times = times_of(info);
    if (::futimens(destination, times.data()) != 0) {
        throw errno_error("set times", to);
    }
#ifdef __APPLE__
    // Darwin ACLs are not xattrs. Replay them after other metadata writes that
    // the source ACL might prohibit.
    if (::fcopyfile(source, destination, nullptr, COPYFILE_ACL) != 0) {
        throw errno_error("set ACL", to);
    }
#endif
}

void replay_symlink_metadata(const std::filesystem::path& from, const std::filesystem::path& to,
                             const struct stat& info) {
    if (::lchown(to.c_str(), info.st_uid, info.st_gid) != 0 && errno != EPERM) {
        throw errno_error("set owner", to);
    }
    copy_xattrs(SymlinkAttributes{from.c_str()}, SymlinkAttributes{to.c_str()}, from, to);
    const auto times = times_of(info);
    if (::utimensat(AT_FDCWD, to.c_str(), times.data(), AT_SYMLINK_NOFOLLOW) != 0) {
        throw errno_error("set times", to);
    }
}

} // namespace hz
