#include "hz/config.hpp"

#include "hz/error.hpp"
#include "hz/fsutil.hpp"

#include <toml++/toml.hpp>

#include <format>
#include <fstream>

namespace hz {

namespace fs = std::filesystem;

namespace {

constexpr std::string_view config_template =
    R"toml(# hz project configuration. This file is copied into every workspace.

[create]
# Omit regenerable artifacts (node_modules, target, .venv, ...) by default.
# filtered = false

[lifecycle]
# Commands are argv arrays run without a shell. A relative executable that
# contains a '/' is resolved from the workspace the hook runs in. Hooks get
# HZ_ROOT, HZ_SOURCE, HZ_WORKSPACE, HZ_WORKSPACE_ID, HZ_PARENT_ID, HZ_HANDLE,
# and HZ_LIFECYCLE in their environment. `--no-hooks` skips them.
#
# Runs in a new workspace once it is ready. A failure is reported, but the
# workspace is kept.
# postcreate = [["./scripts/setup"], ["npm", "install", "--prefer-offline"]]
#
# Runs in each workspace before it moves to trash. A failure cancels removal.
# preremove = [["./scripts/teardown"]]
)toml";

[[noreturn]] void invalid(const fs::path& file, const toml::source_region& where,
                          std::string_view message) {
    throw Error(ErrorKind::invalid_argument,
                std::format("{}:{}: {}", file.string(), where.begin.line, message));
}

std::vector<Command> read_commands(const toml::table& table, std::string_view key,
                                   const fs::path& file) {
    std::vector<Command> commands;
    const toml::node* node = table.get(key);
    if (node == nullptr) {
        return commands;
    }
    const toml::array* list = node->as_array();
    if (list == nullptr) {
        invalid(file, node->source(), std::format("{} must be an array of commands", key));
    }
    for (const toml::node& entry : *list) {
        Command command;
        if (const auto* text = entry.as_string()) {
            // A bare string is an executable with no arguments.
            command.push_back(text->get());
        } else if (const auto* argv = entry.as_array()) {
            for (const toml::node& argument : *argv) {
                const auto* value = argument.as_string();
                if (value == nullptr) {
                    invalid(file, argument.source(), "command arguments must be strings");
                }
                command.push_back(value->get());
            }
        }
        if (command.empty() || command.front().empty()) {
            invalid(file, entry.source(),
                    std::format("each {} entry must be a non-empty array of strings", key));
        }
        commands.push_back(std::move(command));
    }
    return commands;
}

} // namespace

Config load_config(const fs::path& workspace) {
    const fs::path file = workspace / config_path;
    if (!exists_nofollow(file)) {
        return {};
    }
    toml::table document;
    try {
        document = toml::parse_file(file.string());
    } catch (const toml::parse_error& error) {
        invalid(file, error.source(), error.description());
    }
    Config config;
    if (const auto* create = document["create"].as_table()) {
        if (const toml::node* filtered = create->get("filtered")) {
            const auto value = filtered->value<bool>();
            if (!value) {
                invalid(file, filtered->source(), "create.filtered must be true or false");
            }
            config.filtered = value;
        }
    }
    if (const auto* lifecycle = document["lifecycle"].as_table()) {
        config.postcreate = read_commands(*lifecycle, "postcreate", file);
        config.preremove = read_commands(*lifecycle, "preremove", file);
    }
    return config;
}

fs::path write_config_template(const fs::path& workspace) {
    const fs::path file = workspace / config_path;
    if (exists_nofollow(file)) {
        return file;
    }
    std::error_code error;
    fs::create_directories(file.parent_path(), error);
    if (error) {
        throw Error(ErrorKind::io,
                    std::format("create {}: {}", file.parent_path().string(), error.message()),
                    error);
    }
    std::ofstream out(file, std::ios::binary);
    out << config_template;
    if (!out) {
        throw errno_error("write", file);
    }
    return file;
}

} // namespace hz
