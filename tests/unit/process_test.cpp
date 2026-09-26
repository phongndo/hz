#include "hz/error.hpp"
#include "hz/process.hpp"

#include <catch2/catch_test_macros.hpp>

TEST_CASE("process input survives a child closing stdin", "[process]") {
    auto result = hz::run_process({"/bin/sh", "-c", "exit 7"},
                                  {.input = std::string(size_t{1024} * 1024, 'x')});
    REQUIRE(result.exit_code == 7);
}

TEST_CASE("process drains output while supplying large input", "[process]") {
    const std::string input(size_t{1024} * 1024, 'x');
    auto result = hz::run_process(
        {"/bin/sh", "-c", "dd if=/dev/zero bs=65536 count=8 2>/dev/null; cat; printf done >&2"},
        {.input = input});
    REQUIRE(result.ok());
    REQUIRE(result.out == std::string(size_t{512} * 1024, '\0') + input);
    REQUIRE(result.err == "done");
}

TEST_CASE("process reports invalid executables and working directories", "[process]") {
    REQUIRE_THROWS_AS(hz::run_process({}), hz::Error);
    REQUIRE_THROWS_AS(hz::run_process({"/hz-missing-executable"}), hz::Error);
    REQUIRE_THROWS_AS(
        hz::run_process({"/bin/sh", "-c", "exit 0"}, {.cwd = "/hz-missing-directory"}), hz::Error);
}
