#include "hz/error.hpp"

#include <cerrno>
#include <format>

namespace hz {

std::string_view to_string(ErrorKind kind) {
    switch (kind) {
    case ErrorKind::io:
        return "io";
    case ErrorKind::cow_unavailable:
        return "cow_unavailable";
    case ErrorKind::unsupported_entry:
        return "unsupported_entry";
    case ErrorKind::invalid_path:
        return "invalid_path";
    }
    return "unknown";
}

Error::Error(ErrorKind kind, const std::string& message)
    : std::runtime_error(message), kind_(kind) {}

Error::Error(ErrorKind kind, const std::string& message, std::error_code code)
    : std::runtime_error(message), kind_(kind), code_(code) {}

Error errno_error(std::string_view operation, const std::filesystem::path& path, int errnum) {
    std::error_code code(errnum, std::system_category());
    return {ErrorKind::io, std::format("{} {}: {}", operation, path.string(), code.message()),
            code};
}

Error errno_error(std::string_view operation, const std::filesystem::path& path) {
    return errno_error(operation, path, errno);
}

} // namespace hz
