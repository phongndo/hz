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
    REQUIRE(human.exit_code == 15);
    REQUIRE(human.out.empty());
    REQUIRE(human.err.starts_with("hz: "));

    auto machine = f.hz(f.temp.path(), {"--machine", "pwd"});
    REQUIRE(machine.exit_code == 15);
    REQUIRE(machine.json()["error"]["kind"] == "not_found");
    REQUIRE(machine.json()["error"]["retryable"] == false);
    REQUIRE(machine.json()["api_version"] == 1);

    REQUIRE(f.hz(f.temp.path(), {"new", "--no-such-flag"}).exit_code == 2);
    REQUIRE(f.hz(f.temp.path(), {"--help"}).exit_code == 0);
}

TEST_CASE("every JSON document carries the API version") {
    Fixture f;
    REQUIRE(f.init().exit_code == 0);
    REQUIRE(f.ok({"--json", "pwd"}).json()["api_version"] == 1);
    REQUIRE(f.ok({"--json", "ls"}).json()["api_version"] == 1);
    REQUIRE(f.ok({"--json", "doctor"}).json()["api_version"] == 1);
}

TEST_CASE("a workspace busy in another process is a retryable error") {
    Fixture f;
    f.init();
    f.child("held");
    const auto id = f.ok({"--json", "path", "held"}).json()["workspace"]["id"].get<std::string>();
    const auto lease = hz::acquire_lease(f.data, id);
    auto result = f.hz(f.project, {"--json", "rm", "held"});
    REQUIRE(result.exit_code == 18);
    REQUIRE(result.json()["error"]["kind"] == "busy");
    REQUIRE(result.json()["error"]["retryable"] == true);
}

TEST_CASE("labels are attached, changed, and used to select workspaces") {
    Fixture f;
    f.init();
    auto created =
        f.ok({"--json", "new", "--label", "owner=t3", "-l", "thread=42", "agent"}).json();
    REQUIRE(created["workspace"]["handle"] == "agent");
    REQUIRE(created["workspace"]["labels"] == nlohmann::json{{"owner", "t3"}, {"thread", "42"}});
    REQUIRE(f.ok({"--json", "path", "root"}).json()["workspace"]["labels"] ==
            nlohmann::json::object());

    // Children do not inherit labels.
    const auto agent = fs::path(created["workspace"]["path"].get<std::string>());
    REQUIRE(f.hz(agent, {"new", "nested"}).exit_code == 0);
    REQUIRE(f.ok({"--json", "path", "nested"}).json()["workspace"]["labels"].empty());

    auto selected = f.ok({"--json", "ls", "--label", "owner=t3"}).json()["workspaces"];
    REQUIRE(selected.size() == 1);
    REQUIRE(selected[0]["handle"] == "agent");
    REQUIRE(f.ok({"--json", "ls", "-l", "thread"}).json()["workspaces"].size() == 1);
    REQUIRE(f.ok({"--json", "ls", "-l", "owner=other"}).json()["workspaces"].empty());

    f.ok({"label", "agent", "status=review", "--unset", "thread"});
    REQUIRE(f.ok({"label", "agent"}).out == "owner=t3\nstatus=review\n");
    REQUIRE(f.ok({"--json", "ls", "-l", "thread"}).json()["workspaces"].empty());

    SECTION("invalid labels are refused before anything changes") {
        REQUIRE(f.hz(f.project, {"new", "bad", "--label", "no-equals"}).exit_code == 14);
        REQUIRE(f.hz(f.project, {"new", "bad", "-l", "k=1", "-l", "k=2"}).exit_code == 14);
        REQUIRE(f.hz(f.project, {"label", "agent", "k=v", "--unset", "k"}).exit_code == 14);
        REQUIRE(f.hz(f.project, {"path", "bad"}).exit_code == 15);
        REQUIRE(f.ok({"label", "agent"}).out == "owner=t3\nstatus=review\n");
    }
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
    REQUIRE(result.exit_code == 11);
    REQUIRE(result.json()["error"]["kind"] == "cow_unavailable");
    REQUIRE_FALSE(fs::exists(f.project / ".hz-workspace"));
}
