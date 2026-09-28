#include "hz/error.hpp"
#include "hz/fsutil.hpp"
#include "hz/marker.hpp"
#include "hz/ulid.hpp"
#include "hz/workspaces.hpp"

#include <catch2/catch_test_macros.hpp>

#include <algorithm>
#include <atomic>
#include <exception>
#include <optional>
#include <string>
#include <thread>
#include <unistd.h>

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
        return workspaces
            ->create({.source = "",
                      .handle = handle,
                      .into = std::nullopt,
                      .filtered = false,
                      .hooks = false})
            .workspace;
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
        family.workspaces->create({.source = parent.id, .handle = "nested", .hooks = false})
            .workspace;
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

TEST_CASE("doctor reports a replaced trash marker", "[review]") {
    Family family;
    const auto child = family.child("replaced-trash");
    family.workspaces->remove({.target = child.id, .hooks = false});
    const auto trashed = *family.registry().find(child.id);
    write_file(*trashed.trash_path / ".hz-workspace", "invalid marker");
    REQUIRE_FALSE(family.doctor(false).empty());
    REQUIRE_FALSE(family.doctor(true).empty());
    REQUIRE(fs::exists(*trashed.trash_path / "file.txt"));
}

TEST_CASE("doctor preserves ambiguous garbage collection locations", "[review]") {
    Family family;
    const auto child = family.child("duplicate-trash");
    family.workspaces->remove({.target = child.id, .hooks = false});
    const auto trashed = *family.registry().find(child.id);
    const fs::path deleting = trashed.trash_path->string() + ".deleting";
    fs::copy(*trashed.trash_path, deleting, fs::copy_options::recursive);
    REQUIRE_FALSE(family.doctor(true).empty());
    REQUIRE(fs::exists(*trashed.trash_path / "file.txt"));
    REQUIRE(fs::exists(deleting / "file.txt"));
    REQUIRE(family.registry().find(child.id));
}

TEST_CASE("doctor reports an inaccessible storage directory", "[review]") {
    Family family;
    const auto storage = hz::Workspaces::storage_directory(family.root);
    write_file(storage, "not a directory");
    REQUIRE(hz::test::error_kind([&] { family.doctor(false); }) == hz::ErrorKind::io);
    REQUIRE(hz::test::read_file(storage) == "not a directory");
}

TEST_CASE("a create in progress is protected by its lease", "[concurrency]") {
    Family family;
    auto child = family.child("copying");
    child.state = hz::State::creating;
    family.registry().transaction([&] { family.registry().update(child); });
    const auto data = family.temp / "data";
    std::optional<hz::detail::Fd> lease(hz::acquire_lease(data, child.id));

    REQUIRE(family.doctor(true).empty());
    REQUIRE(fs::exists(child.path / "file.txt"));
    REQUIRE(hz::test::error_kind([&] {
                family.workspaces->remove({.target = child.id, .hooks = false});
            }) == hz::ErrorKind::conflict);
    REQUIRE(hz::test::error_kind([&] { family.workspaces->require_quiescent(family.root); }) ==
            hz::ErrorKind::conflict);
    REQUIRE(hz::test::error_kind([&] { family.workspaces->require_quiescent(child); }) ==
            hz::ErrorKind::conflict);

    lease.reset(); // as when the copying process dies
    REQUIRE(has(family.doctor(true), "interrupted_create", true));
    REQUIRE_FALSE(fs::exists(child.path));
    REQUIRE_FALSE(fs::exists(data / "leases" / child.id));
    REQUIRE_NOTHROW(family.workspaces->require_quiescent(family.root));
}

TEST_CASE("gc leaves trash that another process is deleting", "[concurrency]") {
    Family family;
    const auto busy = family.child("busy");
    const auto idle = family.child("idle");
    for (const auto& workspace : {busy, idle}) {
        family.workspaces->remove({.target = workspace.id, .hooks = false});
    }
    const auto data = family.temp / "data";
    {
        const auto lease = hz::acquire_lease(data, busy.id);
        const auto result = family.workspaces->gc();
        REQUIRE(result.deleted.size() == 1);
        REQUIRE(result.deleted.front().id == idle.id);
        REQUIRE(family.registry().find(busy.id));
        REQUIRE(family.doctor(true).empty());
    }
    REQUIRE(family.workspaces->gc().deleted.size() == 1);
    REQUIRE_FALSE(family.registry().find(busy.id));
    REQUIRE_FALSE(fs::exists(hz::Workspaces::storage_directory(family.root)));
}

TEST_CASE("a create by an older hz without a lease is judged by its pid", "[concurrency]") {
    Family family;
    auto child = family.child("older");
    child.state = hz::State::creating;
    child.pid = ::getpid();
    family.registry().transaction([&] { family.registry().update(child); });
    REQUIRE(family.doctor(true).empty());
    REQUIRE(hz::test::error_kind([&] {
                family.workspaces->remove({.target = child.id, .hooks = false});
            }) == hz::ErrorKind::conflict);
    REQUIRE(fs::exists(child.path / "file.txt"));
}

TEST_CASE("changes to a child made while it is copied survive its activation", "[concurrency]") {
    Family family;
    for (int i = 0; i < 4000; ++i) {
        write_file(family.project / "tree" / std::to_string(i % 100) / std::to_string(i), "x");
    }
    hz::Workspaces other(family.temp / "data", family.project);
    // Pinning must land while the copy runs; retry if a copy wins the race.
    for (int attempt = 0; attempt < 5; ++attempt) {
        const std::string handle = "copying" + std::to_string(attempt);
        std::exception_ptr failure;
        std::atomic<bool> done = false;
        std::thread creator([&] {
            try {
                family.child(handle);
            } catch (...) {
                failure = std::current_exception();
            }
            done = true;
        });
        bool pinned = false;
        for (;;) {
            const bool finished = done;
            const auto found = other.registry().find_handle(family.root.id, handle);
            if (!found && finished) {
                break; // the create failed; rethrown below
            }
            if (found && found->state == hz::State::active) {
                break;
            }
            if (found) {
                other.set_pinned(found->id, true);
                pinned = true;
                break;
            }
        }
        creator.join();
        if (failure) {
            std::rethrow_exception(failure);
        }
        if (pinned) {
            const auto child = family.registry().find_handle(family.root.id, handle);
            REQUIRE(child->state == hz::State::active);
            REQUIRE(child->pinned);
            return;
        }
    }
    FAIL("every copy finished before it could be pinned");
}

TEST_CASE("a workspace busy in another process is not removed or rewritten", "[concurrency]") {
    Family family;
    const auto child = family.child("hooked");
    const auto lease = hz::acquire_lease(family.temp / "data", child.id);
    REQUIRE(hz::test::error_kind([&] {
                family.workspaces->remove({.target = child.id, .hooks = false});
            }) == hz::ErrorKind::conflict);
    REQUIRE(hz::test::error_kind([&] { family.workspaces->require_quiescent(child); }) ==
            hz::ErrorKind::conflict);
    REQUIRE(hz::test::error_kind([&] {
                family.workspaces->create({.source = child.id, .handle = "copy", .hooks = false});
            }) == hz::ErrorKind::conflict);
    REQUIRE(family.doctor(true).empty());
    REQUIRE(fs::exists(child.path / "file.txt"));
}

TEST_CASE("doctor --fix removes lease files nobody holds", "[recovery]") {
    Family family;
    const auto data = family.temp / "data";
    const auto child = family.child("done");
    const auto token = hz::generate_ulid();
    for (const auto& id : {child.id, token}) {
        (void)hz::acquire_lease(data, id); // released at once, file left behind
    }
    const auto held_token = hz::generate_ulid();
    const auto held = hz::acquire_lease(data, held_token);
    REQUIRE(fs::exists(data / "leases" / token));
    family.doctor(false);
    REQUIRE(fs::exists(data / "leases" / token));
    family.doctor(true);
    REQUIRE_FALSE(fs::exists(data / "leases" / child.id));
    REQUIRE_FALSE(fs::exists(data / "leases" / token));
    REQUIRE(fs::exists(data / "leases" / held_token));
}
