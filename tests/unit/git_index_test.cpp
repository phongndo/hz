#include "hz/clone.hpp"
#include "hz/detail/digest.hpp"
#include "hz/git.hpp"
#include "hz/process.hpp"
#include "hz/tree.hpp"

#include <catch2/catch_test_macros.hpp>

#include <array>
#include <chrono>
#include <cstdint>
#include <cstdlib>
#include <fcntl.h>
#include <format>
#include <initializer_list>
#include <sstream>
#include <string>
#include <sys/stat.h>
#include <vector>

#include "support.hpp"

using hz::test::TempDir;
using hz::test::write_file;
namespace fs = std::filesystem;

namespace {

template <std::size_t N> std::string hex(const std::array<std::uint8_t, N>& digest) {
    std::string text;
    for (const auto byte : digest) {
        text += std::format("{:02x}", byte);
    }
    return text;
}

std::vector<std::uint8_t> bytes(std::string_view text) {
    return {text.begin(), text.end()};
}

std::string git(const fs::path& repository, std::initializer_list<std::string> args) {
    ::setenv("GIT_CONFIG_GLOBAL", "/dev/null", 1);
    ::setenv("GIT_CONFIG_NOSYSTEM", "1", 1);
    std::vector<std::string> argv{"git",
                                  "-C",
                                  repository.string(),
                                  "-c",
                                  "user.name=hz test",
                                  "-c",
                                  "user.email=hz@example.com"};
    argv.insert(argv.end(), args);
    auto result = hz::run_process(argv);
    INFO("git " << argv[7] << ": " << result.err);
    REQUIRE(result.ok());
    return result.out;
}

// Sets the modification time of `path` `age` before now.
void age(const fs::path& path, std::chrono::seconds age) {
    const auto when = std::chrono::system_clock::now() - age;
    const auto seconds = std::chrono::duration_cast<std::chrono::seconds>(when.time_since_epoch());
    const std::array<timespec, 2> times{timespec{.tv_sec = seconds.count(), .tv_nsec = 0},
                                        timespec{.tv_sec = seconds.count(), .tv_nsec = 0}};
    REQUIRE(::utimensat(AT_FDCWD, path.c_str(), times.data(), AT_SYMLINK_NOFOLLOW) == 0);
}

// A committed repository whose files predate its index, plus a tracked
// file changed afterwards and an untracked file.
fs::path make_repository(const TempDir& temp, std::initializer_list<std::string> init) {
    const fs::path source = temp / "source";
    fs::create_directories(source);
    std::vector<std::string> args{"init", "-q"};
    args.insert(args.end(), init);
    auto argv = std::vector<std::string>{"git", "-C", source.string()};
    argv.insert(argv.end(), args.begin(), args.end());
    REQUIRE(hz::run_process(argv).ok());
    write_file(source / "a.txt", "alpha");
    write_file(source / "dir" / "b.txt", "bravo");
    write_file(source / "run.sh", "#!/bin/sh\n");
    fs::permissions(source / "run.sh", fs::perms::owner_exec, fs::perm_options::add);
    fs::create_symlink("a.txt", source / "link");
    for (const auto* name : {"a.txt", "dir/b.txt", "run.sh", "link"}) {
        age(source / name, std::chrono::hours(1));
    }
    git(source, {"add", "."});
    git(source, {"commit", "-q", "-m", "initial"});
    write_file(source / "dir" / "b.txt", "bravo, changed");
    write_file(source / "new.txt", "untracked");
    return source;
}

// Copies `source` to a child, as create does, and applies `refresh` to it
// with the stat data the copy observed; returns the entries refreshed.
std::size_t copy_and_refresh(const TempDir& temp, const fs::path& source,
                             hz::git::IndexRefresh& refresh) {
    const auto mode = hz::probe_clone_support(temp.path()) ? hz::CopyMode::cow : hz::CopyMode::copy;
    hz::git::SourceStats seen;
    hz::copy_tree(source, temp / "child",
                  {.mode = mode,
                   .clone_whole_tree = true,
                   .observe = [&seen](const std::string& relative, const struct stat& info) {
                       seen.add(relative, info);
                   }});
    return refresh.apply(temp / "child", seen);
}

std::size_t copy_and_refresh(const TempDir& temp, const fs::path& source) {
    hz::git::IndexRefresh refresh(source);
    return copy_and_refresh(temp, source, refresh);
}

// The inode number the index records for `path`, from `git ls-files --debug`.
std::string recorded_inode(const fs::path& repository, const std::string& path) {
    std::istringstream lines(git(repository, {"ls-files", "--debug", "--", path}));
    for (std::string line; std::getline(lines, line);) {
        const auto at = line.find("ino: ");
        if (at != std::string::npos) {
            return line.substr(at + 5);
        }
    }
    return {};
}

std::string inode_of(const fs::path& path) {
    struct stat info{};
    REQUIRE(::lstat(path.c_str(), &info) == 0);
    return std::to_string(static_cast<std::uint32_t>(info.st_ino));
}

} // namespace

TEST_CASE("digests match published test vectors") {
    REQUIRE(hex(hz::detail::sha1(bytes(""))) == "da39a3ee5e6b4b0d3255bfef95601890afd80709");
    REQUIRE(hex(hz::detail::sha1(bytes("abc"))) == "a9993e364706816aba3e25717850c26c9cd0d89d");
    REQUIRE(hex(hz::detail::sha1(bytes(std::string(1000000, 'a')))) ==
            "34aa973cd4c4daa4f61eeb2bdbad27316534016f");
    REQUIRE(hex(hz::detail::sha256(bytes(""))) ==
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
    REQUIRE(hex(hz::detail::sha256(
                bytes("abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"))) ==
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1");
}

TEST_CASE("the index refresh gives entries clean in the source the stat data of their copies") {
    TempDir temp;
    fs::path source;
    SECTION("index version 2") {
        source = make_repository(temp, {});
    }
    SECTION("index version 4") {
        source = make_repository(temp, {});
        git(source, {"update-index", "--index-version", "4"});
    }
    SECTION("SHA-256 repository") {
        source = make_repository(temp, {"--object-format=sha256"});
    }
    // a.txt, run.sh, and link; dir/b.txt changed after it was indexed.
    REQUIRE(copy_and_refresh(temp, source) == 3);
    const auto child = temp / "child";
    REQUIRE(recorded_inode(child, "a.txt") == inode_of(child / "a.txt"));
    REQUIRE(recorded_inode(child, "dir/b.txt") == inode_of(source / "dir" / "b.txt"));
    git(child, {"fsck", "--no-dangling", "--no-progress"});
    REQUIRE(git(child, {"status", "--porcelain"}) == git(source, {"status", "--porcelain"}));
    REQUIRE(git(child, {"status", "--porcelain"}) == " M dir/b.txt\n?? new.txt\n");
}

TEST_CASE("the index refresh leaves racily clean entries for Git to check") {
    TempDir temp;
    const auto source = make_repository(temp, {});
    // Entries no older than the index could have changed unseen.
    age(source / ".git" / "index", std::chrono::hours(2));
    REQUIRE(copy_and_refresh(temp, source) == 0);
    const auto child = temp / "child";
    REQUIRE(git(child, {"status", "--porcelain"}) == git(source, {"status", "--porcelain"}));
}

TEST_CASE("the index refresh leaves an index Git rewrote while the copy was made") {
    TempDir temp;
    const auto source = make_repository(temp, {});
    hz::git::IndexRefresh refresh(source); // read before the copy, as create does
    git(source, {"add", "new.txt"});
    REQUIRE(copy_and_refresh(temp, source, refresh) == 0);
    const auto child = temp / "child";
    REQUIRE(git(child, {"status", "--porcelain"}) == git(source, {"status", "--porcelain"}));
}

TEST_CASE("the index refresh gives up on a source whose index is stale") {
    TempDir temp;
    const auto source = make_repository(temp, {});
    // Recreating every file leaves the index recording none of them.
    for (const auto* name : {"a.txt", "run.sh"}) {
        const auto content = hz::test::read_file(source / name);
        fs::remove(source / name);
        write_file(source / name, content);
    }
    fs::permissions(source / "run.sh", fs::perms::owner_exec, fs::perm_options::add);
    fs::remove(source / "link");
    fs::create_symlink("a.txt", source / "link");
    REQUIRE(copy_and_refresh(temp, source) == 0);
    const auto child = temp / "child";
    REQUIRE(git(child, {"status", "--porcelain"}) == git(source, {"status", "--porcelain"}));
}

TEST_CASE("the index refresh leaves a split index alone") {
    TempDir temp;
    const auto source = make_repository(temp, {});
    git(source, {"update-index", "--split-index"});
    REQUIRE(copy_and_refresh(temp, source) == 0);
    const auto child = temp / "child";
    REQUIRE(git(child, {"status", "--porcelain"}) == git(source, {"status", "--porcelain"}));
}
