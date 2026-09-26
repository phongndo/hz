#include "fixture.hpp"

using hz::test::Fixture;
using hz::test::read_file;
using hz::test::write_file;
namespace fs = std::filesystem;

TEST_CASE("hz new copies the current workspace into a registered child") {
    Fixture f;
    f.init();
    write_file(f.project / "node_modules" / "dep" / "index.js", "dep");

    auto created = f.ok({"--json", "new"}).json()["workspace"];
    const fs::path path = created["path"].get<std::string>();
    REQUIRE(created["state"] == "active");
    REQUIRE(created["parent_id"] == f.ok({"--json", "pwd"}).json()["workspace"]["id"]);
    REQUIRE(read_file(path / "src" / "main.cpp") == "int main() {}");
    REQUIRE(read_file(path / "node_modules" / "dep" / "index.js") == "dep");
    REQUIRE(read_file(path / ".hz-workspace") == created["id"].get<std::string>() + "\n");

    SECTION("children of children share the family storage") {
        auto nested = f.hz(path, {"--json", "new", "nested"}).json()["workspace"];
        REQUIRE(fs::path(nested["path"].get<std::string>()).parent_path() == path.parent_path());
        auto chain =
            f.hz(nested["path"].get<std::string>(), {"--json", "ancestors"}).json()["workspaces"];
        REQUIRE(chain.size() == 2);
        REQUIRE(chain[0]["handle"] == "app");
        REQUIRE(chain[1]["id"] == created["id"]);
    }
    SECTION("handles must be valid and unique in the family") {
        REQUIRE(f.hz(f.project, {"new", "bad/name"}).exit_code == 1);
        f.ok({"new", "taken"});
        REQUIRE(f.hz(f.project, {"--json", "new", "taken"}).json()["error"]["kind"] == "conflict");
    }
}

TEST_CASE("hz new --filtered omits regenerable artifacts, and config sets the default") {
    Fixture f;
    f.init();
    write_file(f.project / "node_modules" / "dep.js", "dep");
    write_file(f.project / "packages" / "a" / "target" / "out.o", "obj");

    REQUIRE(fs::exists(f.child("full") / "node_modules"));
    const fs::path lean = Fixture::trim(f.ok({"new", "--filtered", "--path-only"}).out);
    REQUIRE_FALSE(fs::exists(lean / "node_modules"));
    REQUIRE_FALSE(fs::exists(lean / "packages" / "a" / "target"));
    REQUIRE(fs::exists(lean / "src" / "main.cpp"));

    write_file(f.project / ".hz" / "hz.toml", "[create]\nfiltered = true\n");
    REQUIRE_FALSE(fs::exists(f.child("by-config") / "node_modules"));
    REQUIRE(fs::exists(fs::path(Fixture::trim(f.ok({"new", "--full", "--path-only"}).out)) /
                       "node_modules"));
}

TEST_CASE("hz rm trashes a subtree that hz restore brings back together") {
    Fixture f;
    f.init();
    const auto parent = f.child("parent");
    const auto nested = fs::path(Fixture::trim(f.hz(parent, {"new", "nested", "--path-only"}).out));

    auto removed = f.ok({"--json", "rm", "parent"}).json();
    REQUIRE(removed["trashed"].size() == 2);
    REQUIRE_FALSE(fs::exists(parent));
    REQUIRE_FALSE(fs::exists(nested));
    REQUIRE(f.ok({"--json", "ls"}).json()["workspaces"].size() == 1);

    SECTION("a child removed with its parent cannot be restored alone") {
        auto result = f.hz(f.project, {"--json", "restore", "nested"});
        REQUIRE(result.json()["error"]["kind"] == "conflict");
    }
    SECTION("restoring the parent restores both") {
        auto restored = f.ok({"--json", "restore", "parent"}).json()["restored"];
        REQUIRE(restored.size() == 2);
        REQUIRE(fs::exists(parent / "src" / "main.cpp"));
        REQUIRE(fs::exists(nested / "src" / "main.cpp"));
    }
    SECTION("a handle can be reused, which blocks restoring the old one") {
        f.ok({"new", "parent"});
        REQUIRE(f.hz(f.project, {"--json", "restore", "parent"}).json()["error"]["kind"] ==
                "conflict");
    }
    SECTION("gc deletes trash for good and tidies storage") {
        auto deleted = f.ok({"--json", "gc"}).json()["deleted"];
        REQUIRE(deleted.size() == 2);
        REQUIRE_FALSE(fs::exists(parent.parent_path() / ".trash"));
        REQUIRE(f.hz(f.project, {"restore", "parent"}).exit_code == 1);
    }
}

TEST_CASE("hz rm --children keeps the target") {
    Fixture f;
    f.init();
    const auto parent = f.child("parent");
    REQUIRE(f.hz(parent, {"new", "nested"}).exit_code == 0);
    auto removed = f.ok({"--json", "rm", "parent", "--children"}).json()["trashed"];
    REQUIRE(removed.size() == 1);
    REQUIRE(removed[0]["handle"] == "nested");
    REQUIRE(fs::exists(parent));
}

TEST_CASE("pinned workspaces refuse removal") {
    Fixture f;
    f.init();
    const auto keep = f.child("keep");
    f.ok({"pin", "keep"});
    REQUIRE(f.hz(f.project, {"--json", "rm", "keep"}).json()["error"]["kind"] == "conflict");
    REQUIRE(fs::exists(keep));
    f.ok({"unpin", "keep"});
    f.ok({"rm", "keep"});
    REQUIRE_FALSE(fs::exists(keep));
}

TEST_CASE("removing a root needs --force and keeps its directory") {
    Fixture f;
    f.init();
    const auto child = f.child("child");
    REQUIRE(f.hz(f.project, {"--json", "rm"}).json()["error"]["kind"] == "conflict");

    auto result = f.ok({"--json", "rm", "--force"}).json();
    REQUIRE(result["unregistered"]["handle"] == "app");
    REQUIRE(result["trashed"].size() == 1);
    REQUIRE(fs::exists(f.project / "src" / "main.cpp"));
    REQUIRE_FALSE(fs::exists(f.project / ".hz-workspace"));
    REQUIRE_FALSE(fs::exists(child));
    f.ok({"gc"});
    REQUIRE(f.hz(f.temp.path(), {"--json", "ls"}).json()["workspaces"].empty());
}

TEST_CASE("hz rm --path-only tells the shell where to go") {
    Fixture f;
    f.init();
    const auto parent = f.child("parent");
    const auto nested = fs::path(Fixture::trim(f.hz(parent, {"new", "nested", "--path-only"}).out));
    REQUIRE(f.hz(nested / "src", {"rm", "--path-only"}).out ==
            fs::canonical(parent).string() + "\n");
    REQUIRE(f.hz(f.project, {"rm", "parent", "--path-only"}).out.empty());
}

TEST_CASE("hz new --into stores the child elsewhere on the same filesystem") {
    Fixture f;
    f.init();
    const auto elsewhere = f.temp / "elsewhere";
    auto created = f.ok({"--json", "new", "--into", elsewhere.string()}).json()["workspace"];
    REQUIRE(fs::path(created["path"].get<std::string>()).parent_path() == fs::canonical(elsewhere));
    REQUIRE(f.hz(f.project, {"--json", "new", "--into", (f.project / "inner").string()})
                .json()["error"]["kind"] == "invalid_path");
}

TEST_CASE("hz adopt follows a moved workspace; doctor reports what it cannot fix") {
    Fixture f;
    f.init();
    const auto child = f.child("moved");
    const auto destination = child.parent_path() / "somewhere-else";
    fs::rename(child, destination);

    auto doctor = f.hz(f.project, {"--json", "doctor"});
    REQUIRE(doctor.exit_code == 1);
    REQUIRE(doctor.json()["findings"][0]["kind"] == "missing");

    f.ok({"adopt", destination.string()});
    REQUIRE(f.ok({"path", "moved"}).out == fs::canonical(destination).string() + "\n");
    REQUIRE(f.ok({"doctor"}).out == "No problems found\n");
}

TEST_CASE("a workspace whose directory is gone can still be removed") {
    Fixture f;
    f.init();
    fs::remove_all(f.child("gone"));
    f.ok({"rm", "gone"});
    f.ok({"gc"});
    REQUIRE(f.ok({"--json", "ls"}).json()["workspaces"].size() == 1);
}

TEST_CASE("lifecycle hooks run with the workspace environment") {
    Fixture f;
    f.init();
    write_file(f.project / "hooks" / "record",
               "#!/bin/sh\nprintf '%s %s %s' \"$HZ_LIFECYCLE\" \"$HZ_HANDLE\" "
               "\"$PWD\" > \"$HZ_ROOT/hook-$HZ_LIFECYCLE\"\n");
    fs::permissions(f.project / "hooks" / "record", fs::perms::owner_all);
    write_file(
        f.project / ".hz" / "hz.toml",
        "[lifecycle]\npostcreate = [\"./hooks/record\"]\npreremove = [[\"./hooks/record\"]]\n");

    const auto child = f.child("hooked");
    const auto canonical = fs::canonical(child).string();
    REQUIRE(read_file(f.project / "hook-postcreate") == "postcreate hooked " + canonical);
    f.ok({"rm", "hooked"});
    REQUIRE(read_file(f.project / "hook-preremove") == "preremove hooked " + canonical);

    SECTION("--no-hooks skips them") {
        fs::remove(f.project / "hook-postcreate");
        f.ok({"new", "quiet", "--no-hooks"});
        REQUIRE_FALSE(fs::exists(f.project / "hook-postcreate"));
    }
    SECTION("a failing postcreate keeps the workspace; a failing preremove cancels removal") {
        write_file(f.project / "hooks" / "record", "#!/bin/sh\nexit 3\n");
        auto created = f.hz(f.project, {"--json", "new", "broken"});
        REQUIRE(created.exit_code == 1);
        REQUIRE(created.json()["error"]["kind"] == "hook_failed");
        const auto path = fs::path(Fixture::trim(f.ok({"path", "broken"}).out));
        REQUIRE(fs::exists(path));
        REQUIRE(f.hz(f.project, {"rm", "broken"}).exit_code == 1);
        REQUIRE(fs::exists(path));
        f.ok({"rm", "broken", "--no-hooks"});
    }
}

TEST_CASE("shell integration scripts and completion") {
    Fixture f;
    f.init();
    for (const auto* shell : {"zsh", "bash", "fish"}) {
        REQUIRE(f.ok({"shell", shell}).out.find("__complete") != std::string::npos);
    }
    REQUIRE(f.hz(f.project, {"shell", "tcsh"}).exit_code == 1);

    f.ok({"new", "alpha"});
    f.ok({"new", "beta"});
    f.ok({"rm", "beta"});
    REQUIRE(f.ok({"__complete", "workspace-targets"}).out == "root\napp\nalpha\n");
    REQUIRE(f.ok({"__complete", "trash-targets"}).out == "beta\n");
}

TEST_CASE("hz install adds shell integration once") {
    Fixture f;
    const auto home = f.temp / "home";
    fs::create_directories(home);
    ::setenv("HOME", home.c_str(), 1);
    ::unsetenv("ZDOTDIR");
    f.ok({"install", "bash"});
    f.ok({"install", "bash"});
    REQUIRE(read_file(home / ".bashrc") == "eval \"$(command hz shell bash)\"\n");
    f.ok({"install", "fish"});
    REQUIRE(read_file(home / ".config" / "fish" / "conf.d" / "hz.fish") ==
            "command hz shell fish | source\n");
}

TEST_CASE("configuration creation is explicit and preserves existing configuration") {
    Fixture f;
    f.init();
    REQUIRE_FALSE(fs::exists(f.project / ".hz"));
    f.ok({"config", "init"});
    REQUIRE(fs::exists(f.project / ".hz" / "hz.toml"));
    const std::string custom = "[create]\nfiltered = false\n";
    write_file(f.project / ".hz" / "hz.toml", custom);
    f.ok({"config", "init"});
    REQUIRE(read_file(f.project / ".hz" / "hz.toml") == custom);
    write_file(f.project / "node_modules" / "keep", "dep");
    REQUIRE(fs::exists(f.child("full-default") / "node_modules" / "keep"));
    write_file(f.project / ".hz" / "hz.toml", "[create]\nfiltered = 'invalid'\n");
    REQUIRE(f.hz(f.project, {"--machine", "new"}).json()["error"]["kind"] == "invalid_argument");
}
