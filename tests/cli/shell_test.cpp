#include <cstdlib>

#include "fixture.hpp"

using hz::test::Fixture;
using hz::test::write_file;

TEST_CASE("shell wrappers navigate only when a path is returned", "[shell]") {
    for (const std::string shell : {"bash", "zsh", "fish"}) {
        INFO(shell);
        Fixture f;
        f.init();
        auto script = f.ok({"shell", shell}).out;
        if (shell == "fish") {
            script += R"sh(
hz new child; or exit 11
test "$PWD" != "$HZ_TEST_ROOT"; or exit 12
hz rm; or exit 13
test "$PWD" = "$HZ_TEST_ROOT"; or exit 14
hz restore child; or exit 15
test "$PWD" != "$HZ_TEST_ROOT"; or exit 16
hz cd root; or exit 17
hz rm child; or exit 18
test "$PWD" = "$HZ_TEST_ROOT"; or exit 19
hz new machine --machine >/dev/null; or exit 20
test "$PWD" = "$HZ_TEST_ROOT"; or exit 21
hz --machine new global >/dev/null; or exit 22
test "$PWD" = "$HZ_TEST_ROOT"; or exit 23
)sh";
        } else {
            // zsh reads script aliases while parsing; call the wrapper
            // functions directly so the test is independent of alias options.
            script += shell == "zsh" ? "\nfunction hz { _hz \"$@\"; }\n" : "\n";
            script += R"sh(
hz new child || exit 11
test "$PWD" != "$HZ_TEST_ROOT" || exit 12
hz rm || exit 13
test "$PWD" = "$HZ_TEST_ROOT" || exit 14
hz restore child || exit 15
test "$PWD" != "$HZ_TEST_ROOT" || exit 16
hz cd root || exit 17
hz rm child || exit 18
test "$PWD" = "$HZ_TEST_ROOT" || exit 19
hz new machine --machine >/dev/null || exit 20
test "$PWD" = "$HZ_TEST_ROOT" || exit 21
hz --machine new global >/dev/null || exit 22
test "$PWD" = "$HZ_TEST_ROOT" || exit 23
)sh";
        }
        const auto file = f.temp / "test-shell";
        write_file(file, script);
        const std::string path =
            std::filesystem::path(HZ_BINARY).parent_path().string() + ":" + std::getenv("PATH");
        const auto result = hz::run_process(
            {shell, file.string()},
            {.cwd = f.project,
             .env = {{"PATH", path},
                     {"HZ_DATA_DIR", f.data.string()},
                     {"HZ_TEST_ROOT", std::filesystem::canonical(f.project).string()}}});
        INFO(result.err);
        REQUIRE(result.exit_code == 0);
    }
}
