#include "hz/clone.hpp"

#include <catch2/catch_test_macros.hpp>

#include "../unit/support.hpp"
#include "run.hpp"

using hz::test::run_hz;
using hz::test::TempDir;
using hz::test::write_file;
namespace fs = std::filesystem;

namespace {

// A project directory, a private registry, and init flags that work on the
// filesystem the tests run on.
struct Fixture {
    TempDir temp;
    fs::path data = temp / "data";
    fs::path project = temp / "app";
    std::string init_flag = hz::probe_clone_support(temp.path()) ? "" : "--copy";

    Fixture() { write_file(project / "src" / "main.cpp", "int main() {}"); }

    hz::test::Result hz(const fs::path& cwd, std::initializer_list<std::string> args) const {
        return run_hz(data, cwd, args);
    }
    hz::test::Result init() const {
        return init_flag.empty() ? hz(project, {"init"}) : hz(project, {"init", init_flag});
    }
};

} // namespace

TEST_CASE("hz init registers the current directory") {
    Fixture f;
    auto result = f.init();
    REQUIRE(result.exit_code == 0);
    REQUIRE(result.out.starts_with("Initialized "));
    REQUIRE(fs::exists(f.project / ".hz-workspace"));

    auto again = f.hz(f.project, {"--json", "init"});
    REQUIRE(again.exit_code == 0);
    auto document = again.json();
    REQUIRE(document["created"] == false);
    REQUIRE(document["workspace"]["handle"] == "app");
    REQUIRE(document["workspace"]["root"] == true);
}

TEST_CASE("hz pwd and hz path resolve from subdirectories") {
    Fixture f;
    REQUIRE(f.init().exit_code == 0);
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
    REQUIRE(f.init().exit_code == 0);
    write_file(f.temp / "other" / "file", "x");
    auto other = f.init_flag.empty() ? f.hz(f.temp / "other", {"init"})
                                     : f.hz(f.temp / "other", {"init", f.init_flag});
    REQUIRE(other.exit_code == 0);

    auto inside = f.hz(f.project, {"--json", "ls"}).json()["workspaces"];
    REQUIRE(inside.size() == 1);
    REQUIRE(inside[0]["handle"] == "app");

    auto outside = f.hz(f.temp.path(), {"--json", "ls"}).json()["workspaces"];
    REQUIRE(outside.size() == 2);

    auto table = f.hz(f.project, {"ls"});
    REQUIRE(table.out.starts_with("* app"));
}

TEST_CASE("hz init refuses a filesystem that cannot clone") {
    Fixture f;
    if (f.init_flag.empty()) {
        SKIP("filesystem supports cloning");
    }
    auto result = f.hz(f.project, {"--json", "init"});
    REQUIRE(result.exit_code == 1);
    REQUIRE(result.json()["error"]["kind"] == "cow_unavailable");
    REQUIRE_FALSE(fs::exists(f.project / ".hz-workspace"));
}
