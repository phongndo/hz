#include "hz/handle.hpp"

#include "hz/error.hpp"

#include <algorithm>
#include <array>
#include <format>
#include <random>

namespace hz {

namespace {

constexpr size_t max_handle_length = 64;

constexpr std::array adjectives{
    "amber",  "brisk", "calm",  "clever", "crisp", "dusty",  "eager", "fancy",
    "gentle", "hazy",  "jolly", "keen",   "lucky", "mellow", "misty", "nimble",
    "proud",  "quiet", "rapid", "rustic", "shiny", "silent", "swift", "tidy",
};

constexpr std::array nouns{
    "badger", "cedar",  "comet",  "delta",   "ember",  "falcon",  "fjord",  "grove",
    "harbor", "island", "lagoon", "maple",   "meadow", "otter",   "pebble", "pine",
    "raven",  "reef",   "river",  "sparrow", "summit", "thistle", "willow", "wren",
};

bool is_handle_character(char c) {
    return (c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z') || (c >= '0' && c <= '9') || c == '.' ||
           c == '_' || c == '-';
}

} // namespace

bool is_valid_handle(std::string_view handle) {
    return !handle.empty() && handle.size() <= max_handle_length && handle.front() != '.' &&
           handle.front() != '-' && handle != "root" && handle != "local" &&
           std::ranges::all_of(handle, is_handle_character);
}

void require_valid_handle(std::string_view handle) {
    if (!is_valid_handle(handle)) {
        throw Error(ErrorKind::invalid_argument,
                    std::format("'{}' is not a valid workspace name: use 1-64 letters, digits, "
                                "'.', '_', or '-', not starting with '.' or '-', and not 'root' "
                                "or 'local'",
                                handle));
    }
}

std::string handle_from_name(std::string_view name) {
    std::string handle;
    for (char c : name) {
        handle.push_back(is_handle_character(c) ? c : '-');
    }
    while (!handle.empty() && (handle.front() == '.' || handle.front() == '-')) {
        handle.erase(handle.begin());
    }
    if (handle.size() > max_handle_length) {
        handle.resize(max_handle_length);
    }
    if (!is_valid_handle(handle)) {
        return "workspace";
    }
    return handle;
}

std::string generate_handle(const std::function<bool(std::string_view)>& taken) {
    std::random_device device;
    std::uniform_int_distribution<size_t> adjective(0, adjectives.size() - 1);
    std::uniform_int_distribution<size_t> noun(0, nouns.size() - 1);
    constexpr int attempts = 32;
    std::string candidate;
    for (int attempt = 0; attempt < attempts; ++attempt) {
        candidate = std::format("{}-{}", adjectives.at(adjective(device)), nouns.at(noun(device)));
        if (!taken(candidate)) {
            return candidate;
        }
    }
    for (int suffix = 2;; ++suffix) {
        auto numbered = std::format("{}-{}", candidate, suffix);
        if (!taken(numbered)) {
            return numbered;
        }
    }
}

} // namespace hz
