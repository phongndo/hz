#pragma once

#include <nlohmann/json.hpp>

#include <cerrno>
#include <cstdlib>
#include <filesystem>
#include <fstream>
#include <initializer_list>
#include <sstream>
#include <stdexcept>
#include <string>
#include <sys/wait.h>
#include <unistd.h>
#include <vector>

namespace hz::test {

struct Result {
    int exit_code = -1;
    std::string out;
    std::string err;

    [[nodiscard]] nlohmann::json json() const { return nlohmann::json::parse(out); }
};

// Runs the hz binary under test in `cwd` with HZ_DATA_DIR set to `data`, so
// every test has a private registry.
inline Result run_hz(const std::filesystem::path& data, const std::filesystem::path& cwd,
                     std::initializer_list<std::string> args) {
    const auto out_path = data.parent_path() / ".hz-test-stdout";
    const auto err_path = data.parent_path() / ".hz-test-stderr";
    std::vector<std::string> storage{HZ_BINARY};
    storage.insert(storage.end(), args);
    std::vector<char*> argv;
    argv.reserve(storage.size() + 1);
    for (auto& arg : storage) {
        argv.push_back(arg.data());
    }
    argv.push_back(nullptr);

    const pid_t pid = ::fork();
    if (pid < 0) {
        throw std::runtime_error("fork failed");
    }
    if (pid == 0) {
        if (::chdir(cwd.c_str()) != 0) {
            ::_exit(126);
        }
        ::setenv("HZ_DATA_DIR", data.c_str(), 1);
        FILE* out = std::freopen(out_path.c_str(), "w", stdout);
        FILE* err = std::freopen(err_path.c_str(), "w", stderr);
        if (out == nullptr || err == nullptr) {
            ::_exit(126);
        }
        ::execv(HZ_BINARY, argv.data());
        ::_exit(127);
    }
    int status = 0;
    while (::waitpid(pid, &status, 0) < 0 && errno == EINTR) {
    }
    auto slurp = [](const std::filesystem::path& path) {
        std::ifstream in(path, std::ios::binary);
        std::stringstream buffer;
        buffer << in.rdbuf();
        return buffer.str();
    };
    Result result;
    result.exit_code = WIFEXITED(status) ? WEXITSTATUS(status) : -1;
    result.out = slurp(out_path);
    result.err = slurp(err_path);
    return result;
}

} // namespace hz::test
