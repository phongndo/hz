#include "hz/snapshot.hpp"

#include "hz/detail/fd.hpp"
#include "hz/error.hpp"

#include <algorithm>
#include <cerrno>
#include <fcntl.h>
#include <iterator>
#include <string>
#include <utility>

#ifdef __linux__
#include <linux/btrfs.h>
#include <linux/magic.h>
#include <sys/ioctl.h>
#include <sys/stat.h>
#include <sys/vfs.h>
#endif

namespace hz {

namespace fs = std::filesystem;

std::string_view to_string(Strategy strategy) {
    switch (strategy) {
    case Strategy::snapshot:
        return "snapshot";
    case Strategy::clone:
        return "clone";
    case Strategy::copy:
        return "copy";
    }
    return "unknown";
}

#ifdef __linux__

namespace {

// The inode number of every btrfs subvolume's top directory.
constexpr ino_t subvolume_root_inode = 256;

} // namespace

bool can_snapshot(const fs::path& directory) {
    const int fd = ::open(directory.c_str(), O_RDONLY | O_DIRECTORY | O_CLOEXEC);
    if (fd < 0) {
        return false;
    }
    const detail::Fd open_directory(fd);
    struct statfs filesystem{};
    struct stat info{};
    if (::fstatfs(fd, &filesystem) != 0 ||
        std::cmp_not_equal(filesystem.f_type, BTRFS_SUPER_MAGIC) || ::fstat(fd, &info) != 0 ||
        info.st_ino != subvolume_root_inode) {
        return false;
    }
    // Lists subvolumes whose parent is this one, wherever they sit in it.
    // Needs Linux 4.18; older kernels just walk.
    btrfs_ioctl_get_subvol_rootref_args references{};
    if (::ioctl(fd, BTRFS_IOC_GET_SUBVOL_ROOTREF, &references) != 0) {
        return false; // EOVERFLOW: more nested subvolumes than fit
    }
    return references.num_items == 0;
}

bool snapshot(const fs::path& from, const fs::path& to) {
    const auto source = detail::Fd::open(from, O_RDONLY | O_DIRECTORY, 0, "open directory");
    const auto parent =
        detail::Fd::open(to.parent_path(), O_RDONLY | O_DIRECTORY, 0, "open directory");
    btrfs_ioctl_vol_args_v2 arguments{};
    arguments.fd = source.get();
    const std::string name = to.filename();
    // The kernel's argument structure keeps the name in a union; zeroed
    // above, it stays NUL-terminated.
    auto& target = arguments.name; // NOLINT(cppcoreguidelines-pro-type-union-access)
    if (name.size() >= std::size(target)) {
        throw Error(ErrorKind::invalid_path, "snapshot name is too long: " + name);
    }
    std::ranges::copy(name, std::begin(target));
    if (::ioctl(parent.get(), BTRFS_IOC_SNAP_CREATE_V2, &arguments) == 0) {
        return true;
    }
    if (errno == EPERM || errno == EACCES) {
        return false;
    }
    throw errno_error("snapshot", from);
}

#else

bool can_snapshot(const fs::path& /*directory*/) {
    return false;
}

bool snapshot(const fs::path& /*from*/, const fs::path& /*to*/) {
    return false;
}

#endif

} // namespace hz
