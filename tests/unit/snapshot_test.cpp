#include "hz/clone.hpp"
#include "hz/fsutil.hpp"
#include "hz/marker.hpp"
#include "hz/snapshot.hpp"
#include "hz/workspaces.hpp"

#include <catch2/catch_test_macros.hpp>

#include <cstring>
#include <fcntl.h>
#include <linux/btrfs.h>
#include <string>
#include <sys/ioctl.h>
#include <sys/stat.h>
#include <unistd.h>

#include "support.hpp"

using hz::test::read_file;
using hz::test::TempDir;
using hz::test::write_file;
namespace fs = std::filesystem;

namespace {

// Creates `path` as a btrfs subvolume, or returns false where that is not
// possible.
bool create_subvolume(const fs::path& path) {
    const int parent = ::open(path.parent_path().c_str(), O_RDONLY | O_DIRECTORY | O_CLOEXEC);
    if (parent < 0) {
        return false;
    }
    btrfs_ioctl_vol_args arguments{};
    std::strncpy(arguments.name, path.filename().c_str(), sizeof arguments.name - 1);
    const bool created = ::ioctl(parent, BTRFS_IOC_SUBVOL_CREATE, &arguments) == 0;
    ::close(parent);
    return created;
}

bool is_subvolume(const fs::path& path) {
    struct stat info{};
    return ::stat(path.c_str(), &info) == 0 && info.st_ino == 256;
}

// A project that is a btrfs subvolume, registered as a root.
struct SubvolumeFamily {
    TempDir temp;
    fs::path project = temp / "app";
    std::optional<hz::Workspaces> workspaces;

    bool make() {
        if (!create_subvolume(project)) {
            return false;
        }
        write_file(project / "file.txt", "content");
        workspaces.emplace(temp / "data", project);
        workspaces->init(project, hz::CopyMode::cow);
        return true;
    }

    hz::Created create(const std::string& source, const std::string& handle, bool filtered) {
        return workspaces->create({.source = source,
                                   .handle = handle,
                                   .into = std::nullopt,
                                   .filtered = filtered,
                                   .hooks = false});
    }
};

} // namespace

TEST_CASE("full creates from a subvolume are snapshots with the walk's result") {
    SubvolumeFamily family;
    if (!family.make()) {
        SKIP("cannot create a btrfs subvolume under " << family.temp.path());
    }
    REQUIRE(hz::same_filesystem(family.project, family.temp.path()));
    write_file(family.project / ".git" / "fsmonitor--daemon.ipc", "");

    const auto child = family.create("", "child", false);
    REQUIRE(child.strategy == hz::Strategy::snapshot);
    REQUIRE(is_subvolume(child.workspace.path));
    REQUIRE(hz::read_marker(child.workspace.path) == child.workspace.id);
    REQUIRE(read_file(child.workspace.path / "file.txt") == "content");
    REQUIRE_FALSE(fs::exists(child.workspace.path / ".git" / "fsmonitor--daemon.ipc"));

    const auto grandchild = family.create(child.workspace.id, "grandchild", false);
    REQUIRE(grandchild.strategy == hz::Strategy::snapshot);
    REQUIRE(family.create("", "filtered", true).strategy == hz::Strategy::clone);

    family.workspaces->remove({.target = child.workspace.id});
    family.workspaces->gc();
    REQUIRE_FALSE(fs::exists(child.workspace.path));
    REQUIRE_FALSE(fs::exists(grandchild.workspace.path));
}

TEST_CASE("a source holding another subvolume is walked") {
    SubvolumeFamily family;
    if (!family.make()) {
        SKIP("cannot create a btrfs subvolume under " << family.temp.path());
    }
    // A snapshot would leave a nested subvolume as an empty directory.
    REQUIRE(create_subvolume(family.project / "nested"));
    write_file(family.project / "nested" / "file.txt", "nested");
    const auto child = family.create("", "child", false);
    REQUIRE(child.strategy == hz::Strategy::clone);
    REQUIRE(read_file(child.workspace.path / "nested" / "file.txt") == "nested");
}
