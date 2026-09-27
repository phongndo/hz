#pragma once

#include "hz/clone.hpp"
#include "hz/fsutil.hpp"
#include "hz/process.hpp"
#include "hz/ulid.hpp"

#include <catch2/catch_test_macros.hpp>

#include <cstdlib>
#include <filesystem>
#include <initializer_list>
#include <string>

#include "../unit/support.hpp"
#include "run.hpp"

namespace hz::test {

namespace fs = std::filesystem;

// Git must not read the developer's configuration, and commits need an author.
inline void hermetic_git() {
    ::setenv("GIT_CONFIG_GLOBAL", "/dev/null", 1);
    ::setenv("GIT_CONFIG_NOSYSTEM", "1", 1);
    ::setenv("GIT_AUTHOR_NAME", "hz test", 1);
    ::setenv("GIT_AUTHOR_EMAIL", "hz@example.com", 1);
    ::setenv("GIT_COMMITTER_NAME", "hz test", 1);
    ::setenv("GIT_COMMITTER_EMAIL", "hz@example.com", 1);
}

inline std::string git(const fs::path& repository, std::initializer_list<std::string> args) {
    std::vector<std::string> argv{"git", "-C", repository.string()};
    argv.insert(argv.end(), args);
    auto result = hz::run_process(argv);
    INFO("git " << argv.back() << ": " << result.err);
    REQUIRE(result.ok());
    return result.out;
}

// A project directory `app`, a private registry, and the init flag that works
// on the filesystem the tests run on.
struct Fixture {
    TempDir temp;
    fs::path data = temp / "data";
    fs::path project = temp / "app";
    bool cow = hz::probe_clone_support(temp.path());

    Fixture() {
        hermetic_git();
        write_file(project / "src" / "main.cpp", "int main() {}");
    }

    [[nodiscard]] Result hz(const fs::path& cwd, std::initializer_list<std::string> args) const {
        return run_hz(data, cwd, args);
    }
    // Runs hz in the project and requires success.
    Result ok(std::initializer_list<std::string> args) const {
        auto result = hz(project, args);
        INFO("hz stderr: " << result.err << " stdout: " << result.out);
        REQUIRE(result.exit_code == 0);
        return result;
    }
    Result init() const { return cow ? ok({"init"}) : ok({"init", "--copy"}); }

    // Turns the project into a Git repository with one commit.
    void make_repository() const {
        git(project, {"init", "-q", "-b", "main"});
        write_file(project / ".gitignore", "node_modules\n");
        git(project, {"add", "."});
        git(project, {"commit", "-q", "-m", "initial"});
    }

    // Creates a child with `name` and returns its path.
    fs::path child(const std::string& name) const {
        return fs::path(trim(ok({"new", name, "--path-only"}).out));
    }

    static std::string trim(std::string text) {
        while (!text.empty() && text.back() == '\n') {
            text.pop_back();
        }
        return text;
    }
};

} // namespace hz::test
