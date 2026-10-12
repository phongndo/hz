#include "hz/version.hpp"

#include <cstdlib>
#include <string>

#include "fixture.hpp"

using hz::test::Fixture;
using hz::test::write_file;
namespace fs = std::filesystem;

TEST_CASE("hz update reruns the install script of the running release") {
    Fixture f;
    // The install script's layout: a release directory and a symlink to its bin/hz.
    const fs::path release = f.temp / "releases" / "v0.10.0-test";
    fs::create_directories(release / "bin");
    fs::copy_file(HZ_BINARY, release / "bin" / "hz");
    write_file(release / "share" / "hz" / "installation",
               "release=v0.10.0\ninstall_dir=/opt/hz bin\nreleases_dir=/opt/hz releases\n");
    write_file(release / "share" / "hz" / "install.sh",
               "printf '%s\\n' \"$HZ_INSTALL_DIR\" \"$HZ_RELEASES_DIR\" \"$HZ_VERSION\" "
               "\"$HZ_CURRENT_RELEASE\"\n");
    const fs::path command = f.temp / "bin" / "hz";
    fs::create_directories(command.parent_path());
    fs::create_symlink(release / "bin" / "hz", command);

    const std::string current = std::string("v") + hz::version + "\n";
    auto pinned = hz::run_process({command.string(), "update", "1.2.3"});
    REQUIRE(pinned.ok());
    REQUIRE(pinned.out == "/opt/hz bin\n/opt/hz releases\n1.2.3\n" + current);
    auto latest = hz::run_process({command.string(), "update"});
    REQUIRE(latest.ok());
    REQUIRE(latest.out == "/opt/hz bin\n/opt/hz releases\nlatest\n" + current);
}

namespace {

// Puts a copy of the hz under test at `executable`, where a package manager would.
void install_copy(const fs::path& executable) {
    fs::create_directories(executable.parent_path());
    fs::copy_file(HZ_BINARY, executable);
}

// A stand-in for a package manager that prints the arguments it was given.
void write_tool(const fs::path& file) {
    write_file(file, "#!/bin/sh\necho \"$@\"\n");
    fs::permissions(file, fs::perms::owner_all);
}

} // namespace

TEST_CASE("hz update runs Homebrew for a Homebrew installation") {
    Fixture f;
    const fs::path prefix = f.temp / "homebrew";
    const fs::path command = prefix / "Cellar" / "hz-cli" / "1.2.3" / "bin" / "hz";
    install_copy(command);
    write_tool(prefix / "bin" / "brew");

    auto upgraded = hz::run_process({command.string(), "update"});
    INFO(upgraded.err);
    REQUIRE(upgraded.ok());
    REQUIRE(upgraded.out == "upgrade hz-cli\n");
    REQUIRE(upgraded.err.contains("Updating with: "));
    REQUIRE(upgraded.err.contains("/bin/brew upgrade hz-cli"));

    auto pinned = hz::run_process({command.string(), "update", "1.2.3"});
    REQUIRE(pinned.exit_code == 14);
    REQUIRE(pinned.err.contains("Homebrew installs only the latest release"));
}

TEST_CASE("hz update runs mise for a mise installation") {
    Fixture f;
    const fs::path data = f.temp / "mise";
    const fs::path command = data / "installs" / "github-phongndo-hz" / "1.2.3" / "bin" / "hz";
    install_copy(command);
    const fs::path tools = f.temp / "tools";
    write_tool(tools / "mise");
    const char* path = std::getenv("PATH");
    const hz::ProcessOptions options{
        .env = {{"MISE_DATA_DIR", data.string()},
                {"PATH", tools.string() + ":" + (path != nullptr ? path : "/usr/bin:/bin")}}};

    auto upgraded = hz::run_process({command.string(), "update"}, options);
    INFO(upgraded.err);
    REQUIRE(upgraded.ok());
    REQUIRE(upgraded.out == "upgrade github:phongndo/hz\n");
    REQUIRE(upgraded.err == "Updating with: mise upgrade github:phongndo/hz\n");

    auto pinned = hz::run_process({command.string(), "update", "v0.9.0"}, options);
    REQUIRE(pinned.exit_code == 14);
    REQUIRE(pinned.err.contains("mise use -g github:phongndo/hz@0.9.0"));
}

TEST_CASE("hz update refuses an hz that no installer manages") {
    Fixture f;
    auto human = f.hz(f.temp.path(), {"update"});
    REQUIRE(human.exit_code == 17);
    REQUIRE(human.err.contains("can only update an hz installed by Homebrew"));
    REQUIRE(human.err.contains("docs/installation.md"));

    auto machine = f.hz(f.temp.path(), {"--machine", "update"});
    REQUIRE(machine.exit_code == 14);
    REQUIRE(machine.json()["error"]["kind"] == "invalid_argument");
}
