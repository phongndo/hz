#include "hz/clone.hpp"
#include "hz/error.hpp"
#include "hz/fsutil.hpp"
#include "hz/marker.hpp"
#include "hz/process.hpp"
#include "hz/tree.hpp"

#include <catch2/catch_test_macros.hpp>

#include <chrono>
#include <dirent.h>
#include <fcntl.h>
#include <format>
#include <map>
#include <optional>
#include <set>
#include <sys/resource.h>
#include <sys/stat.h>
#include <sys/xattr.h>
#include <tuple>
#include <unistd.h>
#include <vector>

#ifdef __APPLE__
#include <memory>
#include <sys/acl.h>
#include <type_traits>
#endif

#include "support.hpp"

using hz::test::error_kind;
using hz::test::mtime_ns;
using hz::test::read_file;
using hz::test::TempDir;
using hz::test::write_file;
namespace fs = std::filesystem;

namespace {

// Builds a source tree exercising every supported entry kind.
fs::path make_fixture(const TempDir& temp) {
    fs::path source = temp / "source";
    write_file(source / "README.md", "hello");
    write_file(source / "src" / "main.cpp", "int main() {}");
    write_file(source / "src" / "deep" / "nested" / "file.txt", "deep");
    write_file(source / "empty-dir" / ".keep", "");
    fs::create_directory(source / "really-empty");
    fs::permissions(source / "src" / "main.cpp", fs::perms::owner_read | fs::perms::owner_write);
    fs::permissions(source / "src" / "deep",
                    fs::perms::owner_all | fs::perms::group_read | fs::perms::group_exec);
    fs::create_symlink("README.md", source / "readme-link");
    fs::create_symlink("missing", source / "dangling");
    fs::create_hard_link(source / "README.md", source / "README-hardlink.md");
    fs::last_write_time(source / "README.md",
                        fs::file_time_type::clock::now() - std::chrono::hours(48));
    return source;
}

void check_copy(const fs::path& source, const fs::path& dest) {
    REQUIRE(read_file(dest / "README.md") == "hello");
    REQUIRE(read_file(dest / "src" / "main.cpp") == "int main() {}");
    REQUIRE(read_file(dest / "src" / "deep" / "nested" / "file.txt") == "deep");
    REQUIRE(fs::is_directory(dest / "really-empty"));
    REQUIRE(fs::exists(dest / "empty-dir" / ".keep"));

    REQUIRE(fs::status(dest / "src" / "main.cpp").permissions() ==
            (fs::perms::owner_read | fs::perms::owner_write));
    REQUIRE(fs::status(dest / "src" / "deep").permissions() ==
            (fs::perms::owner_all | fs::perms::group_read | fs::perms::group_exec));
    REQUIRE(fs::status(dest).permissions() == fs::status(source).permissions());

    REQUIRE(fs::is_symlink(dest / "readme-link"));
    REQUIRE(fs::read_symlink(dest / "readme-link") == "README.md");
    REQUIRE(fs::read_symlink(dest / "dangling") == "missing");

    REQUIRE(fs::hard_link_count(dest / "README.md") == 2);
    REQUIRE(fs::equivalent(dest / "README.md", dest / "README-hardlink.md"));
    REQUIRE_FALSE(fs::equivalent(dest / "README.md", source / "README.md"));

    REQUIRE(mtime_ns(dest / "README.md") == mtime_ns(source / "README.md"));
    REQUIRE(mtime_ns(dest / "src") == mtime_ns(source / "src"));
}

} // namespace

namespace {

// Everything copy_tree promises about each entry, keyed by relative path, and
// which entries share an inode.
struct TreeScan {
    std::map<std::string, std::tuple<fs::file_type, fs::perms, std::string, std::int64_t>> entries;
    std::set<std::set<std::string>> links;
};

TreeScan scan(const fs::path& root) {
    TreeScan result;
    std::map<std::pair<dev_t, ino_t>, std::set<std::string>> inodes;
    for (const auto& entry : fs::recursive_directory_iterator(root)) {
        const auto relative = entry.path().lexically_relative(root).string();
        const auto status = fs::symlink_status(entry.path());
        std::string content;
        std::int64_t mtime = 0;
        if (status.type() == fs::file_type::symlink) {
            content = fs::read_symlink(entry.path()).string();
        } else {
            mtime = mtime_ns(entry.path());
            if (status.type() == fs::file_type::regular) {
                content = read_file(entry.path());
                struct stat info{};
                REQUIRE(::lstat(entry.path().c_str(), &info) == 0);
                if (info.st_nlink > 1) {
                    inodes[{info.st_dev, info.st_ino}].insert(relative);
                }
            }
        }
        result.entries[relative] = {status.type(), status.permissions(), content, mtime};
    }
    for (auto& [inode, paths] : inodes) {
        result.links.insert(paths);
    }
    return result;
}

// Many directories, a directory copied in several batches, hard links across
// directories, and read-only directories at depth.
fs::path make_large_fixture(const TempDir& temp) {
    const fs::path source = temp / "source";
    for (int i = 0; i < 40; ++i) {
        const auto directory = source / std::format("d{}", i) / "a" / "b";
        write_file(directory / "file.txt", std::format("file {}", i));
        fs::create_symlink("file.txt", directory / "link");
    }
    for (int i = 0; i < 40; i += 4) {
        fs::create_hard_link(source / std::format("d{}", i) / "a" / "b" / "file.txt",
                             source / std::format("d{}", i + 1) / "hard.txt");
    }
    for (int i = 0; i < 40; i += 7) {
        fs::permissions(source / std::format("d{}", i) / "a" / "b",
                        fs::perms::owner_read | fs::perms::owner_exec);
    }
    for (int i = 0; i < 700; ++i) {
        write_file(source / "flat" / std::format("f{}", i), std::format("flat {}", i));
    }
    fs::permissions(source / "flat" / "f5", fs::perms::owner_read);
    return source;
}

// The first entry whose recorded properties differ, for a readable failure.
std::string first_difference(const TreeScan& actual, const TreeScan& expected) {
    for (const auto& [path, properties] : expected.entries) {
        const auto found = actual.entries.find(path);
        if (found == actual.entries.end()) {
            return "missing " + path;
        }
        const auto& [type, perms, content, mtime] = properties;
        const auto& [actual_type, actual_perms, actual_content, actual_mtime] = found->second;
        if (found->second != properties) {
            return std::format("{}: type {} vs {}, perms {:o} vs {:o}, mtime {} vs {}, content "
                               "equal {}",
                               path, static_cast<int>(actual_type), static_cast<int>(type),
                               static_cast<unsigned>(actual_perms), static_cast<unsigned>(perms),
                               actual_mtime, mtime, actual_content == content);
        }
    }
    for (const auto& [path, properties] : actual.entries) {
        if (!expected.entries.contains(path)) {
            return "unexpected " + path;
        }
    }
    return actual.links == expected.links ? "" : "hard link groups differ";
}

} // namespace

TEST_CASE("copy_tree copies large trees identically with any number of workers") {
    TempDir temp;
    const auto source = make_large_fixture(temp);
    const auto mode = hz::probe_clone_support(temp.path()) ? hz::CopyMode::cow : hz::CopyMode::copy;
    hz::copy_tree(source, temp / "serial", {.mode = mode, .workers = 1});
    hz::copy_tree(source, temp / "parallel", {.mode = mode, .workers = 8});
    const auto expected = scan(source);
    REQUIRE(expected.entries.size() == 911);
    REQUIRE(expected.links.size() == 10);
    REQUIRE(first_difference(scan(temp / "serial"), expected).empty());
    REQUIRE(first_difference(scan(temp / "parallel"), expected).empty());
    REQUIRE_FALSE(fs::equivalent(temp / "parallel" / "d0" / "a" / "b" / "file.txt",
                                 source / "d0" / "a" / "b" / "file.txt"));
}

TEST_CASE("copy_tree keeps walked semantics for subtrees a clone would alter") {
    // On macOS clean subtrees are cloned whole; these nested entries must
    // still come out as the walk copies them.
    TempDir temp;
    const auto mode = hz::probe_clone_support(temp.path()) ? hz::CopyMode::cow : hz::CopyMode::copy;
    const fs::path source = temp / "source";
    write_file(source / "clean" / "a" / "b" / "file.txt", "clean");
    fs::create_symlink("file.txt", source / "clean" / "a" / "b" / "link");
    write_file(source / "linked" / "a" / "one.txt", "linked");
    fs::create_hard_link(source / "linked" / "a" / "one.txt", source / "linked" / "a" / "two.txt");
    write_file(source / "setid" / "a" / "tool", "#!/bin/sh\n");
    // Set-GID needs membership of the file's group, which a temporary
    // directory's group may not give.
    fs::permissions(source / "setid" / "a" / "tool", fs::perms::owner_all | fs::perms::set_uid);
    write_file(source / "skipping" / "a" / "keep.txt", "keep");
    write_file(source / "skipping" / "a" / "cache" / "drop.txt", "drop");
    auto expected = scan(source);
    std::erase_if(expected.entries,
                  [](const auto& entry) { return entry.first.starts_with("skipping/a/cache"); });
    const auto skip = [](const fs::path& relative) { return relative.filename() == "cache"; };
    bool whole = false;
    SECTION("subtree clones") {}
    SECTION("a whole-tree clone, which a skipped directory turns back into a walk") {
        whole = true;
    }

    hz::copy_tree(source, temp / "dest", {.mode = mode, .skip = skip, .clone_whole_tree = whole});

    const auto difference = first_difference(scan(temp / "dest"), expected);
    INFO(difference);
    REQUIRE(difference.empty());
    REQUIRE(expected.links.size() == 1);
    REQUIRE(fs::hard_link_count(temp / "dest" / "linked" / "a" / "one.txt") == 2);
    const auto perms = fs::status(temp / "dest" / "setid" / "a" / "tool").permissions();
    REQUIRE((perms & fs::perms::set_uid) == fs::perms::set_uid);
}

TEST_CASE("a whole-tree clone comes out as the walk would copy the tree") {
    TempDir temp;
    const auto mode = hz::probe_clone_support(temp.path()) ? hz::CopyMode::cow : hz::CopyMode::copy;
    const fs::path source = temp / "source";
    write_file(source / "a" / "b" / "file.txt", "file");
    fs::create_symlink("file.txt", source / "a" / "b" / "link");
    write_file(source / "a" / "one.txt", "linked");
    fs::create_hard_link(source / "a" / "one.txt", source / "a" / "two.txt");
    fs::create_hard_link(source / "a" / "one.txt", source / "b-three.txt");
    write_file(source / "tool", "#!/bin/sh\n");
    fs::permissions(source / "tool", fs::perms::owner_all | fs::perms::set_uid);
    write_file(source / ".hz-workspace", "01M3FT15QDE8TKB66940X4WNKG");
    fs::create_directories(source / "empty");
    for (const auto* directory : {"a/b", "a", "empty"}) {
        fs::last_write_time(source / directory,
                            fs::file_time_type::clock::now() - std::chrono::hours(24));
    }
    // A group of the caller's other than the one the destination would give,
    // so ownership has to be restored.
    std::optional<gid_t> other_group;
    std::vector<gid_t> groups(static_cast<std::size_t>(::getgroups(0, nullptr)));
    ::getgroups(static_cast<int>(groups.size()), groups.data());
    struct stat parent{};
    REQUIRE(::stat(temp.path().c_str(), &parent) == 0);
    for (const auto group : groups) {
        if (group != parent.st_gid) {
            other_group = group;
            break;
        }
    }
    if (other_group) {
        REQUIRE(::lchown((source / "a" / "b" / "file.txt").c_str(), static_cast<uid_t>(-1),
                         *other_group) == 0);
    }
    auto expected = scan(source);
    expected.entries.erase(".hz-workspace");
    const auto skip = [](const fs::path& relative) { return relative == ".hz-workspace"; };

    hz::copy_tree(source, temp / "dest", {.mode = mode, .skip = skip, .clone_whole_tree = true});

    const auto difference = first_difference(scan(temp / "dest"), expected);
    INFO(difference);
    REQUIRE(difference.empty());
    REQUIRE(fs::hard_link_count(temp / "dest" / "a" / "one.txt") == 3);
    const auto perms = fs::status(temp / "dest" / "tool").permissions();
    REQUIRE((perms & fs::perms::set_uid) == fs::perms::set_uid);
    if (other_group) {
        struct stat copied{};
        REQUIRE(::lstat((temp / "dest" / "a" / "b" / "file.txt").c_str(), &copied) == 0);
        REQUIRE(copied.st_gid == *other_group);
    }
}

TEST_CASE("a whole-tree clone refuses special files like the walk") {
    TempDir temp;
    const auto mode = hz::probe_clone_support(temp.path()) ? hz::CopyMode::cow : hz::CopyMode::copy;
    const fs::path source = temp / "source";
    write_file(source / "a" / "file", "1");
    REQUIRE(::mkfifo((source / "a" / "fifo").c_str(), 0600) == 0);
    REQUIRE(error_kind([&] {
                hz::copy_tree(source, temp / "dest", {.mode = mode, .clone_whole_tree = true});
            }) == hz::ErrorKind::unsupported_entry);
    REQUIRE_FALSE(fs::exists(temp / "dest"));
}

TEST_CASE("read_directory lists an open directory completely every time") {
    TempDir temp;
    for (const char* name : {"a", "b", "c"}) {
        write_file(temp / name, name);
    }
    fs::create_directory(temp / "d");
    const auto directory =
        hz::detail::Fd::open(temp.path(), O_RDONLY | O_DIRECTORY, 0, "open directory");
    for (int pass = 0; pass < 2; ++pass) {
        std::set<std::pair<std::string, unsigned char>> seen;
        for (const auto& entry : hz::read_directory(directory.get(), temp.path())) {
            seen.emplace(entry.name, entry.type);
        }
        REQUIRE(seen == std::set<std::pair<std::string, unsigned char>>{
                            {"a", DT_REG}, {"b", DT_REG}, {"c", DT_REG}, {"d", DT_DIR}});
    }
}

TEST_CASE("copy_tree copies under a umask that withholds owner access") {
    TempDir temp;
    write_file(temp / "source" / "nested" / "file", "content");
    const mode_t previous = ::umask(0777);
    std::optional<hz::ErrorKind> failure;
    try {
        hz::copy_tree(temp / "source", temp / "dest", {.mode = hz::CopyMode::copy, .workers = 2});
    } catch (const hz::Error& error) {
        failure = error.kind();
    }
    ::umask(previous);
    REQUIRE_FALSE(failure);
    REQUIRE(read_file(temp / "dest" / "nested" / "file") == "content");
    REQUIRE(fs::status(temp / "dest" / "nested").permissions() ==
            fs::status(temp / "source" / "nested").permissions());
}

TEST_CASE("tree work leaves the open-file limit as it found it") {
    TempDir temp;
    write_file(temp / "source" / "file", "1");
    rlimit before{};
    REQUIRE(::getrlimit(RLIMIT_NOFILE, &before) == 0);
    hz::copy_tree(temp / "source", temp / "dest", {.mode = hz::CopyMode::copy, .workers = 2});
    hz::remove_tree(temp / "dest");
    rlimit after{};
    REQUIRE(::getrlimit(RLIMIT_NOFILE, &after) == 0);
    REQUIRE(after.rlim_cur == before.rlim_cur);
}

TEST_CASE("copy_tree removes a partial parallel copy that fails deep in the tree") {
    TempDir temp;
    const auto source = make_large_fixture(temp);
    REQUIRE(::mkfifo((source / "d20" / "a" / "fifo").c_str(), 0600) == 0);
    const fs::path dest = temp / "dest";
    REQUIRE(error_kind([&] {
                hz::copy_tree(source, dest, {.mode = hz::CopyMode::copy, .workers = 8});
            }) == hz::ErrorKind::unsupported_entry);
    REQUIRE_FALSE(fs::exists(fs::symlink_status(dest)));
}

TEST_CASE("remove_tree deletes read-only and inaccessible directories, marker last") {
    TempDir temp;
    const auto root = make_large_fixture(temp);
    hz::write_marker(root, "01M3FT15QDE8TKB66940X4WNKG");
    fs::create_directories(root / "closed" / "inner");
    write_file(root / "closed" / "inner" / "file", "x");
    fs::permissions(root / "closed" / "inner", fs::perms::none);
    fs::permissions(root / "closed", fs::perms::owner_read | fs::perms::owner_exec);
    fs::permissions(root, fs::perms::owner_read | fs::perms::owner_exec);
    hz::remove_tree(root);
    REQUIRE_FALSE(fs::exists(fs::symlink_status(root)));
    REQUIRE_NOTHROW(hz::remove_tree(root)); // already gone
}

TEST_CASE("worker slots are shared by concurrent holders") {
    TempDir temp;
    std::optional<hz::WorkerSlots> first(std::in_place, temp.path());
    const unsigned all = first->count();
    REQUIRE(all >= 1);
    {
        const hz::WorkerSlots second(temp.path());
        REQUIRE(second.count() == 1); // none free: still one thread
    }
    first.reset();
    REQUIRE(hz::WorkerSlots(temp.path()).count() == all);
}

TEST_CASE("copy_tree reproduces a tree in copy mode") {
    TempDir temp;
    fs::path source = make_fixture(temp);
    fs::path dest = temp / "dest";
    hz::copy_tree(source, dest, {.mode = hz::CopyMode::copy});
    check_copy(source, dest);
}

TEST_CASE("copy_tree reproduces a tree by cloning") {
    TempDir temp;
    if (!hz::probe_clone_support(temp.path())) {
        SKIP("filesystem at " << temp.path() << " cannot clone");
    }
    fs::path source = make_fixture(temp);
    fs::path dest = temp / "dest";
    hz::copy_tree(source, dest, {.mode = hz::CopyMode::cow});
    check_copy(source, dest);

    SECTION("clones are independent once written") {
        write_file(dest / "README.md", "changed");
        REQUIRE(read_file(source / "README.md") == "hello");
    }
}

TEST_CASE("cloning fails explicitly where the filesystem cannot clone") {
    TempDir temp;
    if (hz::probe_clone_support(temp.path())) {
        SKIP("filesystem at " << temp.path() << " supports cloning");
    }
    write_file(temp / "file", "data");
    write_file(temp / "source" / "file", "data");
    REQUIRE(error_kind([&] { hz::clone_file(temp / "file", temp / "clone", hz::CopyMode::cow); }) ==
            hz::ErrorKind::cow_unavailable);
    REQUIRE_FALSE(fs::exists(temp / "clone"));
    REQUIRE(error_kind([&] {
                hz::copy_tree(temp / "source", temp / "dest", {.mode = hz::CopyMode::cow});
            }) == hz::ErrorKind::cow_unavailable);
    REQUIRE_FALSE(fs::exists(temp / "dest"));
}

TEST_CASE("clone_file preserves permission bits including set-id bits") {
    TempDir temp;
    write_file(temp / "file", "data");
    // Sandboxed environments may refuse the set-gid bit; the copy must match
    // whatever bits the source actually carries.
    fs::permissions(temp / "file", fs::perms::owner_all | fs::perms::group_exec);
    std::error_code ignored;
    fs::permissions(temp / "file", fs::perms::set_gid, fs::perm_options::add, ignored);
    const auto bits = fs::status(temp / "file").permissions();
    REQUIRE((bits & fs::perms::group_exec) == fs::perms::group_exec);
    hz::clone_file(temp / "file", temp / "copy", hz::CopyMode::copy);
    REQUIRE(fs::status(temp / "copy").permissions() == bits);
    REQUIRE(read_file(temp / "copy") == "data");
}

TEST_CASE("clone_file refuses to overwrite") {
    TempDir temp;
    write_file(temp / "file", "data");
    write_file(temp / "existing", "keep");
    REQUIRE(error_kind([&] {
                hz::clone_file(temp / "file", temp / "existing", hz::CopyMode::copy);
            }) == hz::ErrorKind::io);
    REQUIRE(read_file(temp / "existing") == "keep");
}

TEST_CASE("copy_tree skips entries and subtrees named by the predicate") {
    TempDir temp;
    fs::path source = temp / "source";
    write_file(source / "keep.txt", "1");
    write_file(source / "node_modules" / "pkg" / "index.js", "2");
    write_file(source / "src" / "node_modules" / "pkg" / "index.js", "3");
    write_file(source / ".hz-workspace", "id");
    fs::path dest = temp / "dest";
    hz::copy_tree(source, dest, {.mode = hz::CopyMode::copy, .skip = [](const fs::path& relative) {
                                     return relative == ".hz-workspace" ||
                                            relative.filename() == "node_modules";
                                 }});
    REQUIRE(fs::exists(dest / "keep.txt"));
    REQUIRE(fs::exists(dest / "src"));
    REQUIRE_FALSE(fs::exists(dest / "node_modules"));
    REQUIRE_FALSE(fs::exists(dest / "src" / "node_modules"));
    REQUIRE_FALSE(fs::exists(dest / ".hz-workspace"));
}

TEST_CASE("copy_tree abandons the copy on an unsupported entry") {
    TempDir temp;
    fs::path source = temp / "source";
    write_file(source / "a" / "file", "1");
    REQUIRE(::mkfifo((source / "a" / "fifo").c_str(), 0600) == 0);
    fs::path dest = temp / "dest";
    REQUIRE(error_kind([&] { hz::copy_tree(source, dest, {.mode = hz::CopyMode::copy}); }) ==
            hz::ErrorKind::unsupported_entry);
    REQUIRE_FALSE(fs::exists(dest));
}

TEST_CASE("copy_tree refuses an existing destination") {
    TempDir temp;
    write_file(temp / "source" / "file", "1");
    write_file(temp / "dest" / "other", "2");
    REQUIRE(error_kind([&] {
                hz::copy_tree(temp / "source", temp / "dest", {.mode = hz::CopyMode::copy});
            }) == hz::ErrorKind::invalid_path);
    REQUIRE(read_file(temp / "dest" / "other") == "2");
}

TEST_CASE("copy_tree replays user extended attributes") {
    TempDir temp;
    fs::path source = temp / "source";
    write_file(source / "file", "1");
#ifdef __APPLE__
    int rc = ::setxattr((source / "file").c_str(), "user.hz", "yes", 3, 0, 0);
#else
    int rc = ::setxattr((source / "file").c_str(), "user.hz", "yes", 3, 0);
#endif
    if (rc != 0) {
        SKIP("filesystem at " << temp.path() << " does not support user xattrs");
    }
    SECTION("writable files keep their attributes") {}
    SECTION("read-only files keep their attributes") {
        fs::permissions(source / "file", fs::perms::owner_read);
    }
    // A umask withholding owner write must not cost the attributes.
    struct Umask {
        mode_t previous = ::umask(0277);
        ~Umask() { ::umask(previous); }
        Umask() = default;
        Umask(const Umask&) = delete;
        Umask& operator=(const Umask&) = delete;
        Umask(Umask&&) = delete;
        Umask& operator=(Umask&&) = delete;
    };
    std::optional<Umask> umask;
    SECTION("under a restrictive umask") {
        umask.emplace();
    }
    auto mode = hz::CopyMode::copy;
    SECTION("byte copies") {}
    SECTION("clones") {
        if (!hz::probe_clone_support(temp.path())) {
            SKIP("filesystem does not support cloning");
        }
        mode = hz::CopyMode::cow;
    }
    hz::copy_tree(source, temp / "dest", {.mode = mode});
    char value[8] = {};
#ifdef __APPLE__
    auto length =
        ::getxattr((temp / "dest" / "file").c_str(), "user.hz", value, sizeof value, 0, 0);
#else
    auto length = ::getxattr((temp / "dest" / "file").c_str(), "user.hz", value, sizeof value);
#endif
    REQUIRE(length == 3);
    REQUIRE(std::string(value, 3) == "yes");
    REQUIRE(fs::status(temp / "dest" / "file").permissions() ==
            fs::status(source / "file").permissions());
}

#ifdef __APPLE__
namespace {

std::string access_acl(const fs::path& path) {
    std::unique_ptr<std::remove_pointer_t<acl_t>, decltype(&acl_free)> acl(
        ::acl_get_file(path.c_str(), ACL_TYPE_EXTENDED), &acl_free);
    REQUIRE(acl);
    std::unique_ptr<char, decltype(&acl_free)> text(::acl_to_text(acl.get(), nullptr), &acl_free);
    REQUIRE(text);
    return text.get();
}

} // namespace

TEST_CASE("copy_tree preserves Darwin file and directory ACLs", "[review]") {
    TempDir temp;
    const auto source = temp / "source";
    write_file(source / "directory" / "file", "content");
    for (const auto& path : {source, source / "directory", source / "directory" / "file"}) {
        REQUIRE(hz::run_process({"/bin/chmod", "+a",
                                 "everyone allow read,readattr,readextattr,readsecurity",
                                 path.string()})
                    .ok());
    }
    auto mode = hz::CopyMode::copy;
    SECTION("byte copies") {}
    bool whole = false;
    SECTION("clones") {
        if (!hz::probe_clone_support(temp.path())) {
            SKIP("filesystem does not support cloning");
        }
        mode = hz::CopyMode::cow;
    }
    SECTION("whole-tree clones") {
        if (!hz::probe_clone_support(temp.path())) {
            SKIP("filesystem does not support cloning");
        }
        mode = hz::CopyMode::cow;
        whole = true;
    }
    const auto dest = temp / "dest";
    hz::copy_tree(source, dest, {.mode = mode, .clone_whole_tree = whole});
    REQUIRE(access_acl(dest) == access_acl(source));
    REQUIRE(access_acl(dest / "directory") == access_acl(source / "directory"));
    REQUIRE(access_acl(dest / "directory" / "file") == access_acl(source / "directory" / "file"));
}
#endif
