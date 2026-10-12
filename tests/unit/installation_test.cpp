#include "hz/installation.hpp"

#include <catch2/catch_test_macros.hpp>

TEST_CASE("the install script's receipt says where it installed hz", "[installation]") {
    const auto found = hz::parse_installation("release=v0.10.0\n"
                                              "install_dir=/home/me/.local/bin\n"
                                              "releases_dir=/home/me/.local/share/hz/releases\n");
    REQUIRE(found);
    REQUIRE(found->install_dir == "/home/me/.local/bin");
    REQUIRE(found->releases_dir == "/home/me/.local/share/hz/releases");

    // Paths keep their spaces and any '=' after the first.
    const auto spaced = hz::parse_installation("install_dir=/a b/bin\nreleases_dir=/a=b\n");
    REQUIRE(spaced);
    REQUIRE(spaced->install_dir == "/a b/bin");
    REQUIRE(spaced->releases_dir == "/a=b");

    REQUIRE_FALSE(hz::parse_installation(""));
    REQUIRE_FALSE(hz::parse_installation("install_dir=/bin\n"));                    // no releases
    REQUIRE_FALSE(hz::parse_installation("install_dir=bin\nreleases_dir=/data\n")); // relative
}
