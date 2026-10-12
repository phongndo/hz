#include "hz/config.hpp"
#include "hz/error.hpp"
#include "hz/filter.hpp"
#include "hz/fsutil.hpp"
#include "hz/git.hpp"
#include "hz/installation.hpp"
#include "hz/process.hpp"
#include "hz/tree.hpp"
#include "hz/version.hpp"

#include <cstdlib>
#include <filesystem>
#include <format>
#include <fstream>
#include <memory>
#include <optional>
#include <print>
#include <sstream>
#include <string>
#include <unistd.h>

#include "cli/commands.hpp"
#include "cli/shell_scripts.hpp"

namespace hz::cli {

namespace fs = std::filesystem;

namespace {

std::string_view script_for(const std::string& shell) {
    if (shell == "zsh") {
        return zsh_script;
    }
    if (shell == "bash") {
        return bash_script;
    }
    if (shell == "fish") {
        return fish_script;
    }
    throw Error(ErrorKind::invalid_argument,
                std::format("unsupported shell '{}': use zsh, bash, or fish", shell));
}

fs::path home() {
    const char* value = std::getenv("HOME");
    if (value == nullptr || *value == '\0') {
        throw Error(ErrorKind::invalid_path, "HOME is not set");
    }
    return value;
}

void add_git_status(Commands& commands, CLI::App* git) {

    auto status_target = std::make_shared<std::string>();
    auto* status = commands.add(
        "status", "Show a workspace's Git status",
        [&commands, status_target] {
            auto workspace = commands.workspaces().resolve(*status_target);
            auto result = git::status(workspace.path);
            const auto output = commands.output();
            if (output.json()) {
                auto entries = nlohmann::json::array();
                for (const auto& entry : result.entries) {
                    entries.push_back({{"code", entry.code}, {"path", entry.path}});
                }
                output.emit({{"workspace", to_json(workspace)},
                             {"branch", result.branch ? nlohmann::json(*result.branch) : nullptr},
                             {"head", result.head ? nlohmann::json(*result.head) : nullptr},
                             {"base", result.base ? nlohmann::json(*result.base) : nullptr},
                             {"dirty", result.dirty()},
                             {"entries", entries}});
                return;
            }
            if (result.branch) {
                output.line(std::format("{}: on branch {}", workspace.handle, *result.branch));
            } else if (result.head) {
                output.line(std::format("{}: HEAD detached at {}", workspace.handle,
                                        result.head->substr(0, 12)));
            } else {
                output.line(std::format("{}: no commits yet", workspace.handle));
            }
            for (const auto& entry : result.entries) {
                output.line(std::format("{} {}", entry.code, entry.path));
            }
        },
        git);
    status->add_option("target", *status_target, "Handle, ID, or path (default: current)");
}

void add_git_handoff(Commands& commands, CLI::App* git) {
    struct HandoffArgs {
        std::string target;
        std::string from;
        bool three_way = false;
        bool path_only = false;
    };
    auto args = std::make_shared<HandoffArgs>();
    auto* handoff = commands.add(
        "handoff", "Apply a workspace's changes to another, by default its parent",
        [&commands, args] {
            const auto lock = lock_operations(default_data_directory());
            auto& workspaces = commands.workspaces();
            auto source = workspaces.resolve(args->from);
            Workspace destination;
            if (!args->target.empty()) {
                destination = workspaces.resolve(args->target);
            } else if (source.parent_id) {
                auto chain = workspaces.ancestors(source);
                destination = chain.back();
            } else {
                throw Error(ErrorKind::invalid_argument,
                            std::format("'{}' is a root and has no parent; name a destination",
                                        source.handle));
            }
            if (destination.id == source.id) {
                throw Error(ErrorKind::invalid_argument,
                            "the source and destination are the same workspace");
            }
            // Handoff writes temporary Git state into both workspaces.
            workspaces.require_quiescent(source);
            workspaces.require_quiescent(destination);
            auto result = git::handoff(source.path, destination.path, args->three_way);
            const auto output = commands.output();
            if (output.json()) {
                output.emit({{"from", to_json(source)},
                             {"to", to_json(destination)},
                             {"base", result.base},
                             {"changed", result.changed}});
            } else if (args->path_only) {
                output.line(destination.path.string());
            } else if (result.changed) {
                output.line(std::format("Applied changes from {} to {}; review them there",
                                        source.handle, destination.handle));
            } else {
                output.line(std::format("{} has no changes to hand off", source.handle));
            }
        },
        git);
    handoff->add_option("target", args->target, "Destination (default: the parent)");
    handoff->add_option("--from", args->from, "Workspace to take changes from (default: current)");
    handoff->add_flag("--3way", args->three_way, "Fall back to a three-way merge");
    handoff->add_flag("--path-only", args->path_only, "Print only the destination's path");
}

void add_hg(Commands& commands) {
    auto* hg = commands.group("hg", "Mercurial operations on workspaces");
    auto target = std::make_shared<std::string>();
    auto* status = commands.add(
        "status", "Show a workspace's Mercurial status",
        [&commands, target] {
            auto workspace = commands.workspaces().resolve(*target);
            const auto text = git::hg_status(workspace.path);
            const auto output = commands.output();
            if (output.json()) {
                output.emit({{"workspace", to_json(workspace)}, {"status", text}});
            } else {
                std::print("{}", text);
            }
        },
        hg);
    status->add_option("target", *target, "Handle, ID, or path (default: current)");
}

void add_config(Commands& commands) {
    auto* config = commands.group("config", "Project configuration");
    auto target = std::make_shared<std::string>();
    auto* init = commands.add(
        "init", "Write a commented .hz/hz.toml",
        [&commands, target] {
            const auto lock = lock_operations(default_data_directory());
            auto& workspaces = commands.workspaces();
            auto workspace = workspaces.resolve(*target);
            workspaces.require_quiescent(workspace);
            const auto file = write_config_template(workspace.path);
            const auto output = commands.output();
            if (output.json()) {
                output.emit({{"path", file.string()}});
            } else {
                output.line(file.string());
            }
        },
        config);
    init->add_option("target", *target, "Workspace (default: current)");
}

std::pair<fs::path, std::string> shell_startup(const std::string& shell) {
    fs::path file;
    std::string line;
    if (shell == "fish") {
        file = home() / ".config" / "fish" / "conf.d" / "hz.fish";
        line = "command hz shell fish | source";
    } else {
        const char* zdotdir = std::getenv("ZDOTDIR");
        file = shell == "zsh"
                   ? fs::path(zdotdir != nullptr && *zdotdir != '\0' ? zdotdir : home()) / ".zshrc"
                   : home() / ".bashrc";
        line = std::format("eval \"$(command hz shell {})\"", shell);
    }
    return {file, line};
}

void add_shell(Commands& commands) {
    auto shell = std::make_shared<std::string>();
    auto* command = commands.add("shell", "Print shell integration for eval",
                                 [shell] { std::print("{}", script_for(*shell)); });
    command->add_option("shell", *shell, "zsh, bash, or fish")->required();

    auto install_shell = std::make_shared<std::string>();
    auto* install = commands.add(
        "install", "Add shell integration to your shell's startup file",
        [&commands, install_shell] {
            (void)script_for(*install_shell); // validate the name
            const auto [file, line] = shell_startup(*install_shell);
            std::string current;
            if (std::ifstream in(file); in) {
                std::stringstream buffer;
                buffer << in.rdbuf();
                current = buffer.str();
            }
            const bool present = current.find(line) != std::string::npos;
            if (!present) {
                fs::create_directories(file.parent_path());
                std::ofstream out(file, std::ios::app);
                out << (current.empty() || current.ends_with('\n') ? "" : "\n") << line << '\n';
                out.flush();
                if (!out) {
                    throw errno_error("write shell integration", file);
                }
            }
            const auto output = commands.output();
            if (output.json()) {
                output.emit({{"path", file.string()}, {"changed", !present}});
            } else {
                output.line(present ? std::format("{} already loads hz", file.string())
                                    : std::format("Added hz to {}; open a new shell to use it",
                                                  file.string()));
            }
        });
    install->add_option("shell", *install_shell, "zsh, bash, or fish")->required();
}

std::optional<Installation> read_receipt(const fs::path& file) {
    std::ifstream in(file);
    if (!in) {
        return std::nullopt;
    }
    std::stringstream receipt;
    receipt << in.rdbuf();
    return parse_installation(receipt.str());
}

// Reruns the install script that came with the running release, in the
// directories its receipt names. Other installations have no receipt.
void add_update(Commands& commands) {
    auto version = std::make_shared<std::string>();
    auto* command = commands.add(
        "update", "Update an hz that the install script installed", [&commands, version] {
            if (commands.output().json()) {
                throw Error(ErrorKind::invalid_argument,
                            "update prints the install script's progress and has no JSON output");
            }
            // The install script links to bin/hz in a release directory, whose
            // share/hz holds the script and its receipt.
            const fs::path executable = current_executable();
            const fs::path share = executable.parent_path().parent_path() / "share" / "hz";
            const auto installation =
                executable.empty() ? std::nullopt : read_receipt(share / "installation");
            if (!installation) {
                throw Error(ErrorKind::conflict,
                            "update only updates an hz that the install script installed; update "
                            "this one the way you installed it:\n"
                            "  brew upgrade hz-cli\n"
                            "  mise upgrade github:phongndo/hz\n"
                            "  nix profile upgrade hz");
            }
            const fs::path script = share / "install.sh";
            const std::string current = std::string("v") + hz::version;
            ::setenv("HZ_INSTALL_DIR", installation->install_dir.c_str(), 1);
            ::setenv("HZ_RELEASES_DIR", installation->releases_dir.c_str(), 1);
            ::setenv("HZ_VERSION", version->empty() ? "latest" : version->c_str(), 1);
            ::setenv("HZ_CURRENT_RELEASE", current.c_str(), 1);
            ::execl("/bin/sh", "sh", script.c_str(), static_cast<char*>(nullptr));
            throw errno_error("run the install script", script);
        });
    command->add_option("version", *version, "A release such as 0.10.0 (default: the latest)");
}

// Hidden entry point for shell completion.
void add_complete(Commands& commands) {
    auto kind = std::make_shared<std::string>();
    auto* command = commands.add("__complete", "Completion candidates", [&commands, kind] {
        const bool trash = *kind == "trash-targets";
        auto rows = commands.workspaces().list({.all_families = false, .include_trashed = trash});
        if (!trash && commands.current_id()) {
            std::println("root");
        }
        for (const auto& workspace : rows) {
            if ((workspace.state == State::trashed) == trash) {
                std::println("{}", workspace.handle);
            }
        }
    });
    command->group("");
    command->add_option("kind", *kind)->required();
}

// Development aid for measuring the walker; not part of the public interface.
void add_copy_tree(Commands& commands) {
    struct Args {
        std::string from;
        std::string to;
        bool copy = false;
        bool filtered = false;
        unsigned workers = 0;
    };
    auto args = std::make_shared<Args>();
    auto* command = commands.add("_copy-tree", "Copy a directory tree", [args] {
        CopyTreeOptions options;
        options.mode = args->copy ? CopyMode::copy : CopyMode::cow;
        if (args->filtered) {
            options.skip = filter_excludes;
        }
        options.workers = args->workers;
        copy_tree(args->from, args->to, options);
    });
    command->group("");
    command->add_option("from", args->from)->required();
    command->add_option("to", args->to)->required();
    command->add_flag("--copy", args->copy);
    command->add_flag("--filtered", args->filtered);
    command->add_option("--workers", args->workers);
}

} // namespace

void add_integration_commands(Commands& commands) {
    auto* git = commands.group("git", "Git operations on workspaces");
    add_git_status(commands, git);
    add_git_handoff(commands, git);
    add_hg(commands);
    add_config(commands);
    add_shell(commands);
    add_update(commands);
    add_complete(commands);
    add_copy_tree(commands);
}

} // namespace hz::cli
