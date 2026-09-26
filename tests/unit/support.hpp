#pragma once

#include "hz/error.hpp"

#include <chrono>
#include <cstdint>
#include <cstdlib>
#include <filesystem>
#include <fstream>
#include <functional>
#include <optional>
#include <sstream>
#include <string>
#include <unistd.h>

namespace hz::test {

namespace fs = std::filesystem;

// Filesystem tests run under HZ_TEST_DIR when set, so the same suite can be
// pointed at btrfs, XFS, APFS, or an ext4 mount that cannot clone.
inline fs::path test_root() {
    if (const char* dir = std::getenv("HZ_TEST_DIR")) {
        return dir;
    }
    return fs::temp_directory_path();
}

class TempDir {
  public:
    TempDir() {
        std::string pattern = (test_root() / "hz-test-XXXXXX").string();
        if (::mkdtemp(pattern.data()) == nullptr) {
            throw errno_error("create temporary directory", pattern);
        }
        path_ = pattern;
    }
    ~TempDir() {
        std::error_code ignored;
        fs::remove_all(path_, ignored);
    }
    TempDir(const TempDir&) = delete;
    TempDir& operator=(const TempDir&) = delete;

    [[nodiscard]] const fs::path& path() const { return path_; }
    fs::path operator/(const char* name) const { return path_ / name; }

  private:
    fs::path path_;
};

inline void write_file(const fs::path& path, const std::string& content) {
    fs::create_directories(path.parent_path());
    std::ofstream out(path, std::ios::binary);
    out << content;
}

// Modification time in nanoseconds. libc++ represents file times with
// __int128, which Catch2 cannot print, so tests compare this instead.
inline std::int64_t mtime_ns(const fs::path& path) {
    return std::chrono::duration_cast<std::chrono::nanoseconds>(
               fs::last_write_time(path).time_since_epoch())
        .count();
}

inline std::string read_file(const fs::path& path) {
    std::ifstream in(path, std::ios::binary);
    std::stringstream buffer;
    buffer << in.rdbuf();
    return buffer.str();
}

// The kind of hz::Error thrown by `action`, or nothing if it did not throw one.
inline std::optional<ErrorKind> error_kind(const std::function<void()>& action) {
    try {
        action();
    } catch (const Error& error) {
        return error.kind();
    }
    return std::nullopt;
}

} // namespace hz::test
