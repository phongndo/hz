#include "hz/filter.hpp"

#include <catch2/catch_test_macros.hpp>

TEST_CASE("filter excludes regenerable artifacts at any depth") {
    REQUIRE(hz::filter_excludes("node_modules"));
    REQUIRE(hz::filter_excludes("packages/app/node_modules/react/index.js"));
    REQUIRE(hz::filter_excludes("target/debug/hz"));
    REQUIRE(hz::filter_excludes("packages/app/.yarn/cache/react.zip"));
    REQUIRE(hz::filter_excludes("packages/app/.yarn/install-state.gz"));
}

TEST_CASE("filter keeps manifests and unrelated yarn files") {
    REQUIRE_FALSE(hz::filter_excludes("packages/app/package-lock.json"));
    REQUIRE_FALSE(hz::filter_excludes("packages/app/.yarn/releases/yarn.cjs"));
    REQUIRE_FALSE(hz::filter_excludes("src/main.cpp"));
}

TEST_CASE("filter never touches source-control metadata except the fsmonitor socket") {
    REQUIRE_FALSE(hz::filter_excludes(".git/build/metadata"));
    REQUIRE_FALSE(hz::filter_excludes("nested/.hg/cache/state"));
    REQUIRE_FALSE(hz::filter_excludes(".jj/repo/store/target"));
    REQUIRE(hz::filter_excludes(".git/fsmonitor--daemon.ipc"));
    REQUIRE(hz::filter_excludes(".git/worktrees/child/fsmonitor--daemon.ipc"));
    REQUIRE_FALSE(hz::filter_excludes(".hg/fsmonitor--daemon.ipc"));
}
