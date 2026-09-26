#include "hz/clone.hpp"

#include "hz/detail/fd.hpp"
#include "hz/error.hpp"
#include "hz/ulid.hpp"

#include <array>
#include <cerrno>
#include <format>
#include <sys/stat.h>
#include <unistd.h>

#ifdef __linux__
#include <linux/fs.h>
#include <sys/ioctl.h>
#elifdef __APPLE__
#include <copyfile.h>
#include <sys/clonefile.h>
#else
#error "hz clone: no clone primitive for this platform"
#endif

namespace hz {

namespace {

constexpr mode_t permission_bits = 07777;

// Whether an errno from the clone primitive means "this filesystem cannot
// clone here" rather than a fault with the particular file.
bool means_cow_unavailable(int errnum) {
    switch (errnum) {
    case ENOTSUP:
    case ENOTTY:
    case EXDEV:
    case EINVAL:
    case ENOSYS:
        return true;
    default:
        return false;
    }
}

Error cow_unavailable_error(const std::filesystem::path& from, int errnum) {
    std::error_code code(errnum, std::system_category());
    return {ErrorKind::cow_unavailable,
            std::format("cannot clone {}: {}", from.string(), code.message()), code};
}

void write_fully(int fd, const char* data, size_t size, const std::filesystem::path& to) {
    while (size > 0) {
        ssize_t written = ::write(fd, data, size);
        if (written < 0) {
            if (errno == EINTR) {
                continue;
            }
            throw errno_error("write", to);
        }
        data += written;
        size -= static_cast<size_t>(written);
    }
}

#ifdef __linux__

void copy_data(int source, int dest, const std::filesystem::path& from,
               const std::filesystem::path& to) {
    // copy_file_range lets the filesystem accelerate the copy (including by
    // sharing blocks); fall back to read/write where it is not supported.
    bool use_range = true;
    std::array<char, 1 << 16> buffer{};
    for (;;) {
        if (use_range) {
            ssize_t copied = ::copy_file_range(source, nullptr, dest, nullptr, 1U << 30, 0);
            if (copied > 0) {
                continue;
            }
            if (copied == 0) {
                return;
            }
            if (errno == EINTR) {
                continue;
            }
            if (errno != EXDEV && errno != EINVAL && errno != ENOSYS && errno != ENOTSUP &&
                errno != EPERM) {
                throw errno_error("copy", from);
            }
            use_range = false;
        }
        ssize_t count = ::read(source, buffer.data(), buffer.size());
        if (count < 0) {
            if (errno == EINTR) {
                continue;
            }
            throw errno_error("read", from);
        }
        if (count == 0) {
            return;
        }
        write_fully(dest, buffer.data(), static_cast<size_t>(count), to);
    }
}

void clone_data(int source, int dest, const std::filesystem::path& from) {
    if (::ioctl(dest, FICLONE, source) == 0) {
        return;
    }
    if (means_cow_unavailable(errno)) {
        throw cow_unavailable_error(from, errno);
    }
    throw errno_error("clone", from);
}

void clone_file_native(const std::filesystem::path& from, const std::filesystem::path& to,
                       CopyMode mode) {
    auto source = detail::Fd::open(from, O_RDONLY | O_NOFOLLOW, 0, "open");
    struct stat info{};
    if (::fstat(source.get(), &info) != 0) {
        throw errno_error("stat", from);
    }
    auto dest =
        detail::Fd::open(to, O_WRONLY | O_CREAT | O_EXCL, info.st_mode & permission_bits, "create");
    try {
        if (mode == CopyMode::cow) {
            clone_data(source.get(), dest.get(), from);
        } else {
            copy_data(source.get(), dest.get(), from, to);
        }
    } catch (...) {
        ::unlink(to.c_str());
        throw;
    }
    // O_CREAT applies the umask; the copy should carry the source's bits.
    if (::fchmod(dest.get(), info.st_mode & permission_bits) != 0) {
        ::unlink(to.c_str());
        throw errno_error("set permissions", to);
    }
}

#elifdef __APPLE__

void clone_file_native(const std::filesystem::path& from, const std::filesystem::path& to,
                       CopyMode mode) {
    struct stat info{};
    if (::lstat(from.c_str(), &info) != 0) {
        throw errno_error("stat", from);
    }
    if (mode == CopyMode::cow) {
        if (::clonefile(from.c_str(), to.c_str(), CLONE_NOFOLLOW | CLONE_ACL) != 0) {
            if (means_cow_unavailable(errno)) {
                throw cow_unavailable_error(from, errno);
            }
            throw errno_error("clone", from);
        }
    } else {
        if (::copyfile(from.c_str(), to.c_str(), nullptr,
                       COPYFILE_DATA | COPYFILE_EXCL | COPYFILE_NOFOLLOW_SRC) != 0) {
            throw errno_error("copy", from);
        }
    }
    // clonefile clears set-ID bits and copyfile applies the umask; restore the
    // source's bits either way.
    if (::chmod(to.c_str(), info.st_mode & permission_bits) != 0) {
        ::unlink(to.c_str());
        throw errno_error("set permissions", to);
    }
}

#endif

} // namespace

void clone_file(const std::filesystem::path& from, const std::filesystem::path& to, CopyMode mode) {
    clone_file_native(from, to, mode);
}

bool probe_clone_support(const std::filesystem::path& directory) {
    auto id = generate_ulid();
    auto source = directory / std::format(".hz-clone-probe-{}", id);
    auto dest = directory / std::format(".hz-clone-probe-copy-{}", id);
    {
        auto fd = detail::Fd::open(source, O_WRONLY | O_CREAT | O_EXCL, 0600, "create");
        write_fully(fd.get(), "hz", 2, source);
    }
    bool supported = false;
    try {
        clone_file(source, dest, CopyMode::cow);
        supported = true;
    } catch (const Error& error) {
        if (error.kind() != ErrorKind::cow_unavailable) {
            ::unlink(source.c_str());
            throw;
        }
    }
    ::unlink(dest.c_str());
    ::unlink(source.c_str());
    return supported;
}

} // namespace hz
