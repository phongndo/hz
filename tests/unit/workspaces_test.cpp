#include "hz/error.hpp"
#include "hz/handle.hpp"
#include "hz/marker.hpp"
#include "hz/registry.hpp"
#include "hz/workspaces.hpp"

#include <catch2/catch_test_macros.hpp>

#include <set>

#include "support.hpp"

using hz::test::error_kind;
using hz::test::TempDir;
using hz::test::write_file;
namespace fs = std::filesystem;

namespace {

hz::Workspace make_row(std::string id, std::string handle, fs::path path,
                       std::optional<std::string> parent = std::nullopt, std::string root = "") {
    hz::Workspace row;
    row.id = std::move(id);
    row.root_id = root.empty() ? row.id : std::move(root);
    row.parent_id = std::move(parent);
    row.handle = std::move(handle);
    row.path = std::move(path);
    row.state = hz::State::active;
    row.created_at = row.updated_at = hz::now_ms();
    return row;
}

} // namespace

TEST_CASE("handles are safe names") {
    REQUIRE(hz::is_valid_handle("parser-fix"));
    REQUIRE(hz::is_valid_handle("v1.2_beta"));
    REQUIRE_FALSE(hz::is_valid_handle(""));
    REQUIRE_FALSE(hz::is_valid_handle(".hidden"));
    REQUIRE_FALSE(hz::is_valid_handle("-flag"));
    REQUIRE_FALSE(hz::is_valid_handle("root"));
    REQUIRE_FALSE(hz::is_valid_handle("local"));
    REQUIRE_FALSE(hz::is_valid_handle("a/b"));
    REQUIRE_FALSE(hz::is_valid_handle("fix(parser)"));
    REQUIRE_FALSE(hz::is_valid_handle(std::string(65, 'a')));
    REQUIRE(hz::handle_from_name("My Project (old)") == "My-Project--old-");
    REQUIRE(hz::handle_from_name(".dotfiles") == "dotfiles");
    REQUIRE(hz::handle_from_name("...") == "workspace");
}

TEST_CASE("generated handles avoid taken names") {
    std::set<std::string> taken;
    for (int i = 0; i < 700; ++i) {
        auto handle =
            hz::generate_handle([&](std::string_view h) { return taken.contains(std::string(h)); });
        REQUIRE(hz::is_valid_handle(handle));
        REQUIRE(taken.insert(handle).second);
    }
}

TEST_CASE("markers round-trip and reject garbage") {
    TempDir temp;
    REQUIRE_FALSE(hz::read_marker(temp.path()));
    hz::write_marker(temp.path(), "01ARZ3NDEKTSV4RRFFQ69G5FAV");
    REQUIRE(hz::read_marker(temp.path()) == "01ARZ3NDEKTSV4RRFFQ69G5FAV");
    fs::create_directories(temp / "a" / "b");
    REQUIRE(hz::find_marked_directory(temp / "a" / "b") == temp.path());
    write_file(temp.path() / ".hz-workspace", "not an id\n");
    REQUIRE(error_kind([&] { (void)hz::read_marker(temp.path()); }) == hz::ErrorKind::inconsistent);
    hz::remove_marker(temp.path());
    REQUIRE_FALSE(fs::exists(temp.path() / ".hz-workspace"));
}

TEST_CASE("registry stores, finds, and walks workspaces") {
    TempDir temp;
    hz::Registry registry(temp / "hz.sqlite");
    auto root = make_row("01AAAAAAAAAAAAAAAAAAAAAAAA", "app", "/code/app");
    auto child = make_row("01BBBBBBBBBBBBBBBBBBBBBBBB", "fix", "/code/.hz/fix", root.id, root.id);
    auto grandchild =
        make_row("01CCCCCCCCCCCCCCCCCCCCCCCC", "follow", "/code/.hz/follow", child.id, root.id);
    registry.transaction([&] {
        registry.insert(root);
        registry.insert(child);
        registry.insert(grandchild);
    });

    REQUIRE(registry.find(child.id)->handle == "fix");
    REQUIRE(registry.find_by_path("/code/app")->id == root.id);
    REQUIRE(registry.find_handle(root.id, "follow")->id == grandchild.id);
    REQUIRE(registry.find_id_prefix("01B").size() == 1);
    REQUIRE(registry.family(root.id).size() == 3);

    auto subtree = registry.subtree(root.id);
    REQUIRE(subtree.size() == 3);
    REQUIRE(subtree.front().id == grandchild.id); // deepest first
    REQUIRE(subtree.back().id == root.id);

    SECTION("handles are unique per family among live workspaces") {
        auto clash =
            make_row("01DDDDDDDDDDDDDDDDDDDDDDDD", "fix", "/code/.hz/other", root.id, root.id);
        REQUIRE(error_kind([&] { registry.transaction([&] { registry.insert(clash); }); }) ==
                hz::ErrorKind::conflict);
        child.state = hz::State::trashed;
        child.trash_path = "/code/.hz/.trash/fix";
        registry.transaction([&] { registry.update(child); });
        registry.transaction([&] { registry.insert(clash); });
        REQUIRE(registry.find_handle(root.id, "fix")->id == clash.id);
    }

    SECTION("a failed transaction leaves nothing behind") {
        auto row = make_row("01EEEEEEEEEEEEEEEEEEEEEEEE", "tmp", "/code/.hz/tmp", root.id, root.id);
        REQUIRE_THROWS(registry.transaction([&] {
            registry.insert(row);
            throw std::runtime_error("abort");
        }));
        REQUIRE_FALSE(registry.find(row.id));
    }
}

TEST_CASE("the registry persists across connections") {
    TempDir temp;
    {
        hz::Registry registry(temp / "hz.sqlite");
        registry.transaction(
            [&] { registry.insert(make_row("01AAAAAAAAAAAAAAAAAAAAAAAA", "app", "/code/app")); });
    }
    hz::Registry reopened(temp / "hz.sqlite");
    REQUIRE(reopened.all().size() == 1);
}

TEST_CASE("incompatible registry versions are rejected without changing their data") {
    TempDir temp;
    {
        hz::sqlite::Database old(temp / "hz.sqlite");
        old.exec("PRAGMA user_version = 1; CREATE TABLE saved (value TEXT); "
                 "INSERT INTO saved VALUES ('keep')");
    }
    REQUIRE(error_kind([&] { hz::Registry old(temp / "hz.sqlite"); }) == hz::ErrorKind::registry);
    hz::sqlite::Database old(temp / "hz.sqlite");
    auto row = old.prepare("SELECT value FROM saved");
    REQUIRE(row.step());
    REQUIRE(row.text(0) == "keep");
}

TEST_CASE("init registers a root and resolves targets") {
    TempDir temp;
    const fs::path project = temp / "project";
    write_file(project / "src" / "main.cpp", "int main() {}");
    hz::Workspaces workspaces(temp / "data", project / "src");
    const auto mode = hz::probe_clone_support(temp.path()) ? hz::CopyMode::cow : hz::CopyMode::copy;

    auto result = workspaces.init(project, mode);
    REQUIRE(result.created);
    REQUIRE(result.workspace.is_root());
    REQUIRE(result.workspace.handle == "project");
    REQUIRE(result.workspace.path == fs::canonical(project));
    REQUIRE(hz::read_marker(project) == result.workspace.id);

    SECTION("init is idempotent") {
        auto again = workspaces.init(project, mode);
        REQUIRE_FALSE(again.created);
        REQUIRE(again.workspace.id == result.workspace.id);
    }
    SECTION("a missing marker is restored by init") {
        hz::remove_marker(project);
        auto again = workspaces.init(project, mode);
        REQUIRE_FALSE(again.created);
        REQUIRE(hz::read_marker(project) == result.workspace.id);
    }
    SECTION("nested roots are refused") {
        REQUIRE(error_kind([&] { workspaces.init(project / "src", mode); }) ==
                hz::ErrorKind::conflict);
    }
    SECTION("targets resolve by current, root, handle, ID, prefix, and path") {
        const auto& id = result.workspace.id;
        REQUIRE(workspaces.current().id == id);
        REQUIRE(workspaces.resolve("").id == id);
        REQUIRE(workspaces.resolve("root").id == id);
        REQUIRE(workspaces.resolve("project").id == id);
        REQUIRE(workspaces.resolve(id).id == id);
        REQUIRE(workspaces.resolve(id.substr(0, 20)).id == id);
        REQUIRE(workspaces.resolve("./").id == id);
        REQUIRE(workspaces.resolve(project.string()).id == id);
        REQUIRE(error_kind([&] { (void)workspaces.resolve("nothing"); }) ==
                hz::ErrorKind::not_found);
    }
    SECTION("a copied directory is not mistaken for the original") {
        fs::copy(project, temp / "copy", fs::copy_options::recursive);
        hz::Workspaces elsewhere(temp / "data", temp / "copy");
        REQUIRE(error_kind([&] { (void)elsewhere.current(); }) == hz::ErrorKind::inconsistent);
    }
}

TEST_CASE("init refuses to clone where the filesystem cannot") {
    TempDir temp;
    if (hz::probe_clone_support(temp.path())) {
        SKIP("filesystem at " << temp.path() << " supports cloning");
    }
    hz::Workspaces workspaces(temp / "data", temp.path());
    fs::create_directory(temp / "project");
    REQUIRE(error_kind([&] { workspaces.init(temp / "project", hz::CopyMode::cow); }) ==
            hz::ErrorKind::cow_unavailable);
    REQUIRE_FALSE(hz::read_marker(temp / "project"));
    REQUIRE(workspaces.init(temp / "project", hz::CopyMode::copy).created);
}
