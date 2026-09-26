#include "fixture.hpp"

using hz::test::Fixture;
using hz::test::write_file;
namespace fs = std::filesystem;

TEST_CASE("hz init registers the current directory") {
    Fixture f;
    auto result = f.init();
    REQUIRE(result.out.starts_with("Initialized "));
    REQUIRE(fs::exists(f.project / ".hz-workspace"));

    auto again = f.ok({"--json", "init"}).json();
    REQUIRE(again["created"] == false);
    REQUIRE(again["workspace"]["handle"] == "app");
    REQUIRE(again["workspace"]["root"] == true);
}

TEST_CASE("hz pwd and hz path resolve from subdirectories") {
    Fixture f;
    f.init();
    const auto canonical = fs::canonical(f.project).string();
    REQUIRE(f.hz(f.project / "src", {"pwd"}).out == canonical + "\n");
    REQUIRE(f.hz(f.project / "src", {"path", "root"}).out == canonical + "\n");
    REQUIRE(f.hz(f.temp.path(), {"path", "app"}).out == canonical + "\n");
    REQUIRE(f.hz(f.temp.path(), {"--at", (f.project / "src").string(), "pwd"}).out ==
            canonical + "\n");
    REQUIRE(f.hz(f.temp.path(), {"cd", "app", "--json"}).json()["workspace"]["path"] == canonical);
}

TEST_CASE("errors are reported on stderr, or as JSON with a kind") {
    Fixture f;
    auto human = f.hz(f.temp.path(), {"pwd"});
    REQUIRE(human.exit_code == 1);
    REQUIRE(human.out.empty());
    REQUIRE(human.err.starts_with("hz: "));

    auto machine = f.hz(f.temp.path(), {"--machine", "pwd"});
    REQUIRE(machine.exit_code == 1);
    REQUIRE(machine.json()["error"]["kind"] == "not_found");
}

TEST_CASE("hz ls lists the current family, or every family outside one") {
    Fixture f;
    f.init();
    write_file(f.temp / "other" / "file", "x");
    auto other =
        f.cow ? f.hz(f.temp / "other", {"init"}) : f.hz(f.temp / "other", {"init", "--copy"});
    REQUIRE(other.exit_code == 0);

    auto inside = f.ok({"--json", "ls"}).json()["workspaces"];
    REQUIRE(inside.size() == 1);
    REQUIRE(inside[0]["handle"] == "app");
    REQUIRE(f.hz(f.temp.path(), {"--json", "ls"}).json()["workspaces"].size() == 2);
    REQUIRE(f.ok({"ls"}).out.starts_with("* app"));
}

TEST_CASE("hz init refuses a filesystem that cannot clone") {
    Fixture f;
    if (f.cow) {
        SKIP("filesystem supports cloning");
    }
    auto result = f.hz(f.project, {"--json", "init"});
    REQUIRE(result.exit_code == 1);
    REQUIRE(result.json()["error"]["kind"] == "cow_unavailable");
    REQUIRE_FALSE(fs::exists(f.project / ".hz-workspace"));
}
