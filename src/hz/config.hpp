#pragma once

#include <filesystem>
#include <optional>
#include <string>
#include <vector>

namespace hz {

// A command as an argv array. Relative executables that contain a '/' are
// resolved from the workspace the hook runs in; others are found on PATH.
using Command = std::vector<std::string>;

// Per-project configuration from <workspace>/.hz/hz.toml. The file is
// optional and is copied into children like any other file.
struct Config {
    std::optional<bool> filtered; // [create] filtered: default for `hz new`
    std::vector<Command> postcreate;
    std::vector<Command> preremove;
};

inline constexpr std::string_view config_path = ".hz/hz.toml";

// Throws Error(invalid_argument) naming the file and line on a bad config.
Config load_config(const std::filesystem::path& workspace);

// Writes a commented template unless the file exists; returns its path.
std::filesystem::path write_config_template(const std::filesystem::path& workspace);

} // namespace hz
