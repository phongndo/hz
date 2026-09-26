#pragma once

#include <filesystem>
#include <stdexcept>
#include <string>
#include <string_view>
#include <system_error>

namespace hz {

// Stable classification of failures, reported verbatim in machine output.
enum class ErrorKind {
    io,                // an operating-system call failed
    cow_unavailable,   // the filesystem cannot clone here
    unsupported_entry, // a socket, fifo, or device cannot be copied
    invalid_path,      // the caller named a path hz cannot use
};

std::string_view to_string(ErrorKind kind);

class Error : public std::runtime_error {
  public:
    Error(ErrorKind kind, const std::string& message);
    Error(ErrorKind kind, const std::string& message, std::error_code code);

    [[nodiscard]] ErrorKind kind() const noexcept { return kind_; }
    [[nodiscard]] const std::error_code& code() const noexcept { return code_; }

  private:
    ErrorKind kind_;
    std::error_code code_;
};

// Builds an io error describing `operation` on `path` from an errno value.
Error errno_error(std::string_view operation, const std::filesystem::path& path, int errnum);
Error errno_error(std::string_view operation, const std::filesystem::path& path);

} // namespace hz
