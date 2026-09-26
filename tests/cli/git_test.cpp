#include "fixture.hpp"

using hz::test::Fixture;
using hz::test::git;
using hz::test::read_file;
using hz::test::write_file;
namespace fs = std::filesystem;

TEST_CASE("children are independent repositories with a detached HEAD") {
    Fixture f;
    f.make_repository();
    f.init();
    const std::string head = Fixture::trim(git(f.project, {"rev-parse", "HEAD"}));

    const auto child = f.child("agent");
    REQUIRE(Fixture::trim(read_file(child / ".git" / "HEAD")) == head);
    REQUIRE(Fixture::trim(git(child, {"rev-parse", "refs/hz/base"})) == head);
    REQUIRE(git(child, {"status", "--porcelain"}).empty()); // marker is excluded
    REQUIRE(git(f.project, {"status", "--porcelain"}).empty());
    REQUIRE(Fixture::trim(git(f.project, {"symbolic-ref", "--short", "HEAD"})) == "main");

    auto status = f.hz(child, {"--json", "git", "status"}).json();
    REQUIRE(status["branch"].is_null());
    REQUIRE(status["head"] == head);
    REQUIRE(status["base"] == head);
    REQUIRE(status["dirty"] == false);
}

TEST_CASE("sources mid-operation or linked worktrees are refused") {
    Fixture f;
    f.make_repository();
    f.init();
    write_file(f.project / ".git" / "index.lock", "");
    REQUIRE(f.hz(f.project, {"--json", "new"}).json()["error"]["kind"] == "unsafe_source");
    fs::remove(f.project / ".git" / "index.lock");
    fs::create_directory(f.project / ".git" / "rebase-merge");
    REQUIRE(f.hz(f.project, {"--json", "new"}).json()["error"]["kind"] == "unsafe_source");
}

TEST_CASE("handoff applies commits, edits, and new files to the parent") {
    Fixture f;
    f.make_repository();
    f.init();
    const auto child = f.child("agent");
    write_file(child / "src" / "main.cpp", "int main() { return 1; }");
    write_file(child / "src" / "committed.cpp", "// committed only");
    git(child, {"add", "src/committed.cpp"});
    git(child, {"commit", "-q", "-am", "agent commit"});
    write_file(child / "src" / "main.cpp", "int main() { return 2; }");
    write_file(child / "src" / "new.cpp", "// new");

    auto handoff = f.hz(child, {"--json", "git", "handoff"}).json();
    REQUIRE(handoff["changed"] == true);
    REQUIRE(handoff["to"]["handle"] == "app");
    REQUIRE(read_file(f.project / "src" / "main.cpp") == "int main() { return 2; }");
    REQUIRE(read_file(f.project / "src" / "new.cpp") == "// new");
    REQUIRE(read_file(f.project / "src" / "committed.cpp") == "// committed only");
    REQUIRE(Fixture::trim(git(f.project, {"symbolic-ref", "--short", "HEAD"})) == "main");

    SECTION("the destination must be clean") {
        auto again = f.hz(child, {"--json", "git", "handoff"});
        REQUIRE(again.json()["error"]["kind"] == "conflict");
    }
}

TEST_CASE("handoff with nothing changed is a no-op") {
    Fixture f;
    f.make_repository();
    f.init();
    const auto child = f.child("idle");
    REQUIRE(f.hz(child, {"--json", "git", "handoff"}).json()["changed"] == false);
}

TEST_CASE("handoff includes commits made after an unborn child was created", "[git-regression]") {
    Fixture f;
    git(f.project, {"init", "-q", "-b", "main"});
    f.init();
    const auto child = f.child("unborn");
    git(child, {"add", "src"});
    git(child, {"commit", "-q", "-m", "first"});
    fs::remove_all(f.project / "src");

    REQUIRE(f.hz(child, {"--json", "git", "handoff"}).json()["changed"] == true);
    REQUIRE(read_file(f.project / "src" / "main.cpp") == "int main() {}");
}

TEST_CASE("reftable sources are refused without invoking Git", "[git-regression]") {
    Fixture f;
    f.make_repository();
    f.init();
    fs::create_directory(f.project / ".git" / "reftable");
    REQUIRE(f.hz(f.project, {"--json", "new"}).json()["error"]["kind"] == "unsafe_source");
}

TEST_CASE("linked worktrees and ref locks are refused", "[git-regression]") {
    Fixture f;
    f.make_repository();
    f.init();
    SECTION("linked worktree") {
        const auto linked = f.temp / "linked";
        git(f.project, {"worktree", "add", "--detach", linked.string()});
        REQUIRE(f.hz(linked, {"init", "--copy"}).exit_code == 0);
        REQUIRE(f.hz(linked, {"--json", "new"}).json()["error"]["kind"] == "unsafe_source");
    }
    SECTION("reference lock") {
        write_file(f.project / ".git" / "refs" / "heads" / "main.lock", "");
        REQUIRE(f.hz(f.project, {"--json", "new"}).json()["error"]["kind"] == "unsafe_source");
    }
}

TEST_CASE("default creation and removal work without SCM executables", "[git-regression]") {
    Fixture f;
    f.make_repository();
    f.init();
    git(f.project, {"pack-refs", "--all"});
    const auto head = git(f.project, {"rev-parse", "HEAD"});
    write_file(f.project / ".git" / "fsmonitor--daemon.ipc", "ephemeral");
    const hz::ProcessOptions options{
        .cwd = f.project, .env = {{"HZ_DATA_DIR", f.data.string()}, {"PATH", "/hz-empty-path"}}};
    auto created = hz::run_process({HZ_BINARY, "new", "no-scm", "--path-only"}, options);
    INFO(created.err);
    REQUIRE(created.ok());
    const fs::path child = Fixture::trim(created.out);
    REQUIRE(read_file(child / ".git" / "HEAD") == head);
    REQUIRE_FALSE(fs::exists(child / ".git" / "fsmonitor--daemon.ipc"));
    REQUIRE(hz::run_process({HZ_BINARY, "rm", "no-scm"}, options).ok());
}
