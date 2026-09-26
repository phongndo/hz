#include "hz/clone.hpp"
#include "hz/error.hpp"
#include "hz/process.hpp"
#include "hz/tree.hpp"

#include <catch2/catch_test_macros.hpp>

#include <chrono>
#include <sys/stat.h>
#include <sys/xattr.h>

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
    hz::copy_tree(source, temp / "dest", {.mode = hz::CopyMode::copy});
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
    SECTION("clones") {
        if (!hz::probe_clone_support(temp.path())) {
            SKIP("filesystem does not support cloning");
        }
        mode = hz::CopyMode::cow;
    }
    const auto dest = temp / "dest";
    hz::copy_tree(source, dest, {.mode = mode});
    REQUIRE(access_acl(dest) == access_acl(source));
    REQUIRE(access_acl(dest / "directory") == access_acl(source / "directory"));
    REQUIRE(access_acl(dest / "directory" / "file") == access_acl(source / "directory" / "file"));
}
#endif
