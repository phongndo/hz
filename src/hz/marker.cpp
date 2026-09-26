#include "hz/marker.hpp"

#include "hz/error.hpp"
#include "hz/ulid.hpp"

#include <cerrno>
#include <format>
#include <fstream>
#include <sstream>
#include <sys/stat.h>
#include <unistd.h>

namespace hz {

namespace fs = std::filesystem;

void write_marker(const fs::path& directory, std::string_view id) {
    const fs::path marker = directory / marker_name;
    const fs::path temporary = directory / std::format("{}.{}.tmp", marker_name, generate_ulid());
    {
        std::ofstream out(temporary, std::ios::binary | std::ios::trunc);
        out << id << '\n';
        out.flush();
        if (!out) {
            std::error_code ignored;
            fs::remove(temporary, ignored);
            throw errno_error("write marker", temporary);
        }
    }
    if (::rename(temporary.c_str(), marker.c_str()) != 0) {
        const int error = errno;
        ::unlink(temporary.c_str());
        throw errno_error("install marker", marker, error);
    }
}

std::optional<std::string> read_marker(const fs::path& directory) {
    const fs::path marker = directory / marker_name;
    struct stat info{};
    if (::lstat(marker.c_str(), &info) != 0) {
        if (errno == ENOENT || errno == ENOTDIR) {
            return std::nullopt;
        }
        throw errno_error("read marker", marker);
    }
    if (!S_ISREG(info.st_mode)) {
        throw Error(ErrorKind::inconsistent,
                    std::format("{} is not a regular file", marker.string()));
    }
    std::ifstream in(marker, std::ios::binary);
    std::stringstream buffer;
    buffer << in.rdbuf();
    std::string id = buffer.str();
    while (!id.empty() && (id.back() == '\n' || id.back() == '\r' || id.back() == ' ')) {
        id.pop_back();
    }
    if (!is_ulid(id)) {
        throw Error(ErrorKind::inconsistent,
                    std::format("{} does not contain a workspace ID", marker.string()));
    }
    return id;
}

void remove_marker(const fs::path& directory) {
    const fs::path marker = directory / marker_name;
    if (::unlink(marker.c_str()) != 0 && errno != ENOENT) {
        throw errno_error("remove marker", marker);
    }
}

std::optional<fs::path> find_marked_directory(const fs::path& start) {
    for (fs::path directory = start;; directory = directory.parent_path()) {
        struct stat info{};
        if (::lstat((directory / marker_name).c_str(), &info) == 0 && S_ISREG(info.st_mode)) {
            return directory;
        }
        if (directory == directory.parent_path()) {
            return std::nullopt;
        }
    }
}

} // namespace hz
