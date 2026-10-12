#include "hz/version.hpp"

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

TEST_CASE("hz update leaves other installations to the tool that made them") {
    Fixture f;
    auto human = f.hz(f.temp.path(), {"update"});
    REQUIRE(human.exit_code == 17);
    REQUIRE(human.err.find("brew upgrade hz-cli") != std::string::npos);

    auto machine = f.hz(f.temp.path(), {"--machine", "update"});
    REQUIRE(machine.exit_code == 14);
    REQUIRE(machine.json()["error"]["kind"] == "invalid_argument");
}
