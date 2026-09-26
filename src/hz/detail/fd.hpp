#pragma once

#include "hz/error.hpp"

#include <fcntl.h>
#include <filesystem>
#include <string_view>
#include <unistd.h>
#include <utility>

namespace hz::detail {

// Owns a POSIX file descriptor.
class Fd {
  public:
    explicit Fd(int fd) noexcept : fd_(fd) {}
    ~Fd() {
        if (fd_ >= 0) {
            ::close(fd_);
        }
    }
    Fd(const Fd&) = delete;
    Fd& operator=(const Fd&) = delete;
    Fd(Fd&& other) noexcept : fd_(std::exchange(other.fd_, -1)) {}
    Fd& operator=(Fd&& other) noexcept {
        std::swap(fd_, other.fd_);
        return *this;
    }

    [[nodiscard]] int get() const noexcept { return fd_; }

    static Fd open(const std::filesystem::path& path, int flags, mode_t mode,
                   std::string_view operation) {
        int fd = ::open(path.c_str(), flags | O_CLOEXEC, mode);
        if (fd < 0) {
            throw errno_error(operation, path);
        }
        return Fd(fd);
    }

  private:
    int fd_;
};

} // namespace hz::detail
