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
#include <cstring>
#include <filesystem>
#include <format>
#include <fstream>
#include <memory>
#include <optional>
#include <print>
#include <sstream>
#include <string>
#include <string_view>
#include <unistd.h>
#include <vector>

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

constexpr std::string_view installation_docs =
    "https://github.com/phongndo/hz/blob/main/docs/installation.md";

// Reruns the install script that came with the running release, in the
// directories its receipt names.
[[noreturn]] void rerun_install_script(const fs::path& share, const Installation& installation,
                                       const std::string& version) {
    const fs::path script = share / "install.sh";
    const std::string current = std::string("v") + hz::version;
    ::setenv("HZ_INSTALL_DIR", installation.install_dir.c_str(), 1);
    ::setenv("HZ_RELEASES_DIR", installation.releases_dir.c_str(), 1);
    ::setenv("HZ_VERSION", version.empty() ? "latest" : version.c_str(), 1);
    ::setenv("HZ_CURRENT_RELEASE", current.c_str(), 1);
    ::execl("/bin/sh", "sh", script.c_str(), static_cast<char*>(nullptr));
    throw errno_error("run the install script", script);
}

// Replaces hz with the package manager's command, so it can replace hz's files.
[[noreturn]] void run_instead(std::vector<std::string> argv) {
    std::string command_line;
    for (const auto& argument : argv) {
        command_line += (command_line.empty() ? "" : " ") + argument;
    }
    std::println(stderr, "Updating with: {}", command_line);
    std::vector<char*> pointers;
    pointers.reserve(argv.size() + 1);
    for (auto& argument : argv) {
        pointers.push_back(argument.data());
    }
    pointers.push_back(nullptr);
    ::execvp(pointers.front(), pointers.data());
    const int error = errno;
    throw Error(ErrorKind::io, std::format("could not run {}: {}; run it yourself: `{}`",
                                           argv.front(), std::strerror(error), command_line));
}

// The element of the default Nix profile that holds `store_path`, if nix can say.
std::optional<std::string> nix_profile_name(const fs::path& store_path) {
    try {
        const auto listed = run_process({"nix", "profile", "list", "--json"});
        return listed.ok() ? nix_profile_element(listed.out, store_path) : std::nullopt;
    } catch (const Error&) {
        return std::nullopt; // no nix on PATH
    }
}

// MISE_DATA_DIR with symlinks resolved, as they are in the executable's path.
std::string mise_data_dir() {
    const char* value = std::getenv("MISE_DATA_DIR");
    if (value == nullptr || *value == '\0') {
        return {};
    }
    std::error_code error;
    const auto resolved = fs::weakly_canonical(value, error);
    return error ? value : resolved.string();
}

// Updates the running hz with whatever installed it: reruns the install
// script, or replaces itself with Homebrew, mise or Nix.
[[noreturn]] void update(const std::string& version) {
    // The install script links to bin/hz in a release directory, whose share/hz
    // holds the script and its receipt.
    const fs::path executable = current_executable();
    const fs::path share = executable.parent_path().parent_path() / "share" / "hz";
    const auto receipt = executable.empty() ? std::nullopt : read_receipt(share / "installation");
    const auto found = installed_by(executable, mise_data_dir(), receipt.has_value());
    // Package managers name a release 0.10.0; the install script takes v0.10.0 too.
    const std::string_view bare =
        std::string_view(version).substr(version.starts_with('v') ? 1 : 0);
    switch (found.installer) {
    case Installer::install_script:
        if (receipt) {
            rerun_install_script(share, *receipt, version);
        }
        break;
    case Installer::homebrew: {
        if (!version.empty()) {
            throw Error(ErrorKind::invalid_argument,
                        "Homebrew installs only the latest release; run `hz update` without a "
                        "version");
        }
        const fs::path brew = found.homebrew_prefix / "bin" / "brew";
        run_instead({::access(brew.c_str(), X_OK) == 0 ? brew.string() : "brew", "upgrade",
                     found.homebrew_formula});
    }
    case Installer::mise:
        if (!version.empty()) {
            throw Error(
                ErrorKind::invalid_argument,
                std::format("pin a version with mise instead: `mise use -g github:phongndo/hz@{}`",
                            bare));
        }
        run_instead({"mise", "upgrade", "github:phongndo/hz"});
    case Installer::nix: {
        // `nix profile upgrade` with a name the profile lacks succeeds without
        // doing anything, so only the profile can name hz.
        const auto name = nix_profile_name(found.nix_store_path);
        if (!name) {
            throw Error(ErrorKind::conflict,
                        "this hz comes from a Nix configuration or `nix run`; update its flake "
                        "input (`nix flake update hz`) and rebuild");
        }
        if (!version.empty()) {
            throw Error(ErrorKind::invalid_argument,
                        std::format("pin a version with Nix instead: `nix profile remove {} && "
                                    "nix profile add github:phongndo/hz/v{}`",
                                    *name, bare));
        }
        run_instead({"nix", "profile", "upgrade", *name});
    }
    case Installer::other:
        break;
    }
    throw Error(ErrorKind::conflict,
                std::format("`hz update` can only update an hz installed by Homebrew, the install "
                            "script, mise or Nix; rebuild this one from source, or reinstall it "
                            "with one of those: {}",
                            installation_docs));
}

void add_update(Commands& commands) {
    auto version = std::make_shared<std::string>();
    auto* command =
        commands.add("update", "Update hz the way it was installed", [&commands, version] {
            if (commands.output().json()) {
                throw Error(ErrorKind::invalid_argument,
                            "update prints its progress as text and has no JSON output");
            }
            update(*version);
        });
    command->add_option("version", *version,
                        "A release such as 0.10.0, for the install script (default: the latest)");
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
