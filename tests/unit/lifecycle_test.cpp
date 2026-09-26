#include "hz/error.hpp"
#include "hz/marker.hpp"
#include "hz/ulid.hpp"
#include "hz/workspaces.hpp"

#include <catch2/catch_test_macros.hpp>

#include <algorithm>

#include "support.hpp"

using hz::test::TempDir;
using hz::test::write_file;
namespace fs = std::filesystem;

namespace {

struct Family {
    TempDir temp;
    fs::path project = temp / "app";
    std::optional<hz::Workspaces> workspaces;
    hz::Workspace root;

    Family() {
        write_file(project / "file.txt", "content");
        workspaces.emplace(temp / "data", project);
        const auto mode =
            hz::probe_clone_support(temp.path()) ? hz::CopyMode::cow : hz::CopyMode::copy;
        root = workspaces->init(project, mode).workspace;
    }

    hz::Workspace child(const std::string& handle) {
        return workspaces->create({.source = "",
                                   .handle = handle,
                                   .into = std::nullopt,
                                   .filtered = false,
                                   .hooks = false});
    }

    std::vector<hz::Finding> doctor(bool fix) { return workspaces->doctor(fix); }
    hz::Registry& registry() { return workspaces->registry(); }
};

bool has(const std::vector<hz::Finding>& findings, const std::string& kind, bool fixed) {
    return std::ranges::any_of(findings, [&](const hz::Finding& finding) {
        return finding.kind == kind && finding.fixed == fixed;
    });
}

} // namespace

TEST_CASE("doctor finishes an interrupted create by discarding it") {
    Family family;
    auto partial = family.child("partial");
    partial.state = hz::State::creating;
    partial.pid = 0; // no such process
    family.registry().transaction([&] { family.registry().update(partial); });

    REQUIRE(has(family.doctor(false), "interrupted_create", false));
    REQUIRE(fs::exists(partial.path));
    REQUIRE(has(family.doctor(true), "interrupted_create", true));
    REQUIRE_FALSE(fs::exists(partial.path));
    REQUIRE_FALSE(family.registry().find(partial.id));
    REQUIRE(family.doctor(false).empty());
}

TEST_CASE("doctor completes an interrupted remove") {
    Family family;
    auto workspace = family.child("half-removed");
    // The registry was updated but the rename never happened.
    workspace.state = hz::State::trashed;
    workspace.trash_path = workspace.path.parent_path() / ".trash" / workspace.id;
    family.registry().transaction([&] { family.registry().update(workspace); });

    REQUIRE(has(family.doctor(true), "interrupted_remove", true));
    REQUIRE(fs::exists(*workspace.trash_path));
    REQUIRE_FALSE(fs::exists(workspace.path));
    REQUIRE(family.workspaces->restore(workspace.id).front().id == workspace.id);
    REQUIRE(fs::exists(workspace.path));
}

TEST_CASE("doctor completes an interrupted restore") {
    Family family;
    auto workspace = family.child("half-restored");
    family.workspaces->remove(
        {.target = workspace.id, .children_only = false, .force = false, .hooks = false});
    auto trashed = *family.registry().find(workspace.id);
    // The registry says active, but the directory is still in trash.
    trashed.state = hz::State::active;
    family.registry().transaction([&] { family.registry().update(trashed); });

    REQUIRE(has(family.doctor(true), "interrupted_restore", true));
    REQUIRE(fs::exists(workspace.path));
    REQUIRE_FALSE(family.registry().find(workspace.id)->trash_path);
    REQUIRE(family.doctor(false).empty());
}

TEST_CASE("doctor finishes an interrupted gc and removes provable orphans") {
    Family family;
    auto workspace = family.child("doomed");
    family.workspaces->remove(
        {.target = workspace.id, .children_only = false, .force = false, .hooks = false});
    const auto trashed = *family.registry().find(workspace.id);
    fs::rename(*trashed.trash_path, trashed.trash_path->string() + ".deleting");
    REQUIRE(has(family.doctor(true), "interrupted_gc", true));
    REQUIRE_FALSE(family.registry().find(workspace.id));

    // A directory in storage carrying the marker of an unregistered ID.
    const auto storage = hz::Workspaces::storage_directory(family.root);
    const auto orphan_id = hz::generate_ulid();
    fs::create_directories(storage / orphan_id);
    hz::write_marker(storage / orphan_id, orphan_id);
    // One without a matching marker is reported but left alone.
    const auto stranger_id = hz::generate_ulid();
    fs::create_directories(storage / stranger_id);

    auto findings = family.doctor(true);
    REQUIRE(has(findings, "orphan", true));
    REQUIRE(has(findings, "orphan", false));
    REQUIRE_FALSE(fs::exists(storage / orphan_id));
    REQUIRE(fs::exists(storage / stranger_id));
}

TEST_CASE("remove refuses to move a directory that is not the workspace") {
    Family family;
    auto workspace = family.child("swapped");
    hz::write_marker(workspace.path, hz::generate_ulid());
    REQUIRE(
        hz::test::error_kind([&] {
            family.workspaces->remove(
                {.target = workspace.id, .children_only = false, .force = false, .hooks = false});
        }) == hz::ErrorKind::inconsistent);
    REQUIRE(fs::exists(workspace.path));
    REQUIRE(family.registry().find(workspace.id)->state == hz::State::active);
}

TEST_CASE("gc preserves the registry of an interrupted remove", "[recovery]") {
    Family family;
    auto workspace = family.child("pending");
    workspace.state = hz::State::trashed;
    workspace.trash_path = workspace.path.parent_path() / ".trash" / workspace.id;
    family.registry().transaction([&] { family.registry().update(workspace); });

    REQUIRE(hz::test::error_kind([&] { family.workspaces->gc(); }) == hz::ErrorKind::inconsistent);
    REQUIRE(family.registry().find(workspace.id));
    REQUIRE(fs::exists(workspace.path / "file.txt"));
    REQUIRE(has(family.doctor(true), "interrupted_remove", true));
    REQUIRE(family.workspaces->restore(workspace.id).front().id == workspace.id);
}

TEST_CASE("gc and doctor preserve a replaced deleting directory", "[recovery]") {
    Family family;
    auto workspace = family.child("replaced");
    family.workspaces->remove({.target = workspace.id, .hooks = false});
    auto trashed = *family.registry().find(workspace.id);
    const fs::path deleting = trashed.trash_path->string() + ".deleting";
    fs::rename(*trashed.trash_path, deleting);
    hz::write_marker(deleting, hz::generate_ulid());

    REQUIRE(hz::test::error_kind([&] { family.workspaces->gc(); }) == hz::ErrorKind::inconsistent);
    REQUIRE_FALSE(family.doctor(true).empty());
    REQUIRE(fs::exists(deleting / "file.txt"));
    REQUIRE(family.registry().find(workspace.id));
}

TEST_CASE("a failed subtree rename rolls back earlier moves", "[recovery]") {
    Family family;
    const auto parent = family.child("parent");
    const auto nested =
        family.workspaces->create({.source = parent.id, .handle = "nested", .hooks = false});
    // Deepest-first removal moves nested before hitting this occupied path.
    const auto occupied = parent.path.parent_path() / ".trash" / parent.id;
    write_file(occupied / "keep", "unrelated");
    REQUIRE_THROWS(family.workspaces->remove({.target = parent.id, .hooks = false}));
    REQUIRE(fs::exists(parent.path / "file.txt"));
    REQUIRE(fs::exists(nested.path / "file.txt"));
    REQUIRE(hz::test::read_file(occupied / "keep") == "unrelated");
    REQUIRE(family.registry().find(parent.id)->state == hz::State::active);
    REQUIRE(family.registry().find(nested.id)->state == hz::State::active);
}

TEST_CASE("doctor leaves an interrupted create with a malformed marker", "[recovery]") {
    Family family;
    auto workspace = family.child("malformed");
    workspace.state = hz::State::creating;
    workspace.pid = 0;
    family.registry().transaction([&] { family.registry().update(workspace); });
    write_file(workspace.path / ".hz-workspace", "not an ID");
    REQUIRE(has(family.doctor(true), "interrupted_create", false));
    REQUIRE(fs::exists(workspace.path / "file.txt"));
    REQUIRE(family.registry().find(workspace.id));
}
