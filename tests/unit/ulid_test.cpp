#include "hz/ulid.hpp"

#include <catch2/catch_test_macros.hpp>

#include <chrono>
#include <set>

using namespace std::chrono_literals;

TEST_CASE("generated ulids are 26 canonical characters") {
    auto id = hz::generate_ulid();
    REQUIRE(id.size() == 26);
    REQUIRE(hz::is_ulid(id));
}

TEST_CASE("the first ten characters encode the millisecond timestamp") {
    REQUIRE(hz::generate_ulid(0ms).substr(0, 10) == "0000000000");
    REQUIRE(hz::generate_ulid(1ms).substr(0, 10) == "0000000001");
    REQUIRE(hz::generate_ulid(32ms).substr(0, 10) == "0000000010");
    // 2^48 - 1 milliseconds is the largest representable timestamp.
    REQUIRE(hz::generate_ulid(std::chrono::milliseconds{(1LL << 48) - 1}).substr(0, 10) ==
            "7ZZZZZZZZZ");
}

TEST_CASE("ulids generated in the same millisecond differ") {
    std::set<std::string> ids;
    for (int i = 0; i < 1000; ++i) {
        ids.insert(hz::generate_ulid(0ms));
    }
    REQUIRE(ids.size() == 1000);
}

TEST_CASE("is_ulid rejects malformed identities") {
    REQUIRE_FALSE(hz::is_ulid(""));
    REQUIRE_FALSE(hz::is_ulid("01ARZ3NDEKTSV4RRFFQ69G5FA"));   // 25 chars
    REQUIRE_FALSE(hz::is_ulid("01ARZ3NDEKTSV4RRFFQ69G5FAVX")); // 27 chars
    REQUIRE_FALSE(hz::is_ulid("01ARZ3NDEKTSV4RRFFQ69G5FAI"));  // I is not Crockford
    REQUIRE_FALSE(hz::is_ulid("01arz3ndektsv4rrffq69g5fav"));  // lowercase
    REQUIRE_FALSE(hz::is_ulid("81ARZ3NDEKTSV4RRFFQ69G5FAV"));  // timestamp overflow
    REQUIRE(hz::is_ulid("01ARZ3NDEKTSV4RRFFQ69G5FAV"));
}
