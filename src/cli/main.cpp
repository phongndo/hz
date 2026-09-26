#include "hz/error.hpp"
#include "hz/filter.hpp"
#include "hz/tree.hpp"
#include "hz/version.hpp"
#include "hz/workspaces.hpp"

#include <CLI/CLI.hpp>

#include <exception>
#include <filesystem>
#include <functional>
#include <map>
#include <memory>
#include <optional>
#include <print>
#include <string>

#include "cli/output.hpp"

namespace fs = std::filesystem;

namespace hz::cli {

namespace {

struct Globals {
    bool json = false;
    bool machine = false;
    std::string at;
};

// Registers subcommands and remembers which action belongs to which, so the
// chosen action runs once parsing (including trailing global flags) is done.
class Commands {
  public:
    explicit Commands(CLI::App& app) : app_(app) {
        app_.add_flag("--json,-j", globals_.json, "Print JSON");
        app_.add_flag("--machine", globals_.machine,
                      "Print JSON and never ask the shell to change directory");
        app_.add_option("--at,-a", globals_.at, "Resolve the current workspace from DIR")
            ->type_name("DIR");
    }

    CLI::App* add(const std::string& name, const std::string& description,
                  std::function<void()> action) {
        auto* command = app_.add_subcommand(name, description);
        command->fallthrough();
        actions_[command] = std::move(action);
        return command;
    }

    int run() {
        const Output output(json());
        for (auto& [command, action] : actions_) {
            if (command->parsed()) {
                try {
                    action();
                    return 0;
                } catch (const Error& error) {
                    output.error(error);
                    return 1;
                }
            }
        }
        std::print("{}", app_.help());
        return 0;
    }

    [[nodiscard]] bool json() const { return globals_.json || globals_.machine; }
    [[nodiscard]] Output output() const { return Output(json()); }

    // Opened lazily so that --help and --version never touch the registry.
    Workspaces& workspaces() {
        if (!workspaces_) {
            fs::path context = globals_.at.empty() ? fs::current_path() : fs::path(globals_.at);
            workspaces_.emplace(default_data_directory(), fs::absolute(context));
        }
        return *workspaces_;
    }

    std::optional<std::string> current_id() {
        try {
            if (auto here = workspaces().current_optional()) {
                return here->id;
            }
        } catch (const Error&) {
            return std::nullopt; // an inconsistent context has no current workspace
        }
        return std::nullopt;
    }

  private:
    CLI::App& app_;
    Globals globals_;
    std::map<CLI::App*, std::function<void()>> actions_;
    std::optional<Workspaces> workspaces_;
};

void print_workspace(Commands& commands, const Workspace& workspace) {
    const auto output = commands.output();
    if (output.json()) {
        output.emit({{"workspace", to_json(workspace)}});
    } else {
        output.line(workspace.path.string());
    }
}

void add_init(Commands& commands) {
    struct Args {
        std::string path;
        bool copy = false;
        bool path_only = false;
    };
    auto args = std::make_shared<Args>();
    auto* command =
        commands.add("init", "Register a directory as a workspace root", [&commands, args] {
            fs::path target =
                args->path.empty() ? commands.workspaces().context() : fs::path(args->path);
            auto result =
                commands.workspaces().init(target, args->copy ? CopyMode::copy : CopyMode::cow);
            const auto output = commands.output();
            if (output.json()) {
                output.emit(
                    {{"workspace", to_json(result.workspace)}, {"created", result.created}});
            } else if (args->path_only) {
                output.line(result.workspace.path.string());
            } else {
                output.line(std::format(
                    "{} {} ({})", result.created ? "Initialized" : "Already initialized",
                    result.workspace.path.string(),
                    result.workspace.mode == CopyMode::cow ? "copy-on-write" : "byte copies"));
            }
        });
    command->add_option("path", args->path, "Directory to register (default: current)");
    command->add_flag("--copy", args->copy,
                      "Use byte copies on filesystems that cannot clone (slower, uses space)");
    command->add_flag("--path-only", args->path_only, "Print only the workspace path");
}

void add_list(Commands& commands) {
    struct Args {
        bool tree = false;
        bool all = false;
        bool trash = false;
    };
    auto args = std::make_shared<Args>();
    auto* command = commands.add("list", "List workspaces", [&commands, args] {
        auto rows =
            commands.workspaces().list({.all_families = args->all, .include_trashed = args->trash});
        const auto output = commands.output();
        if (output.json()) {
            output.emit({{"workspaces", to_json(rows)}});
        } else if (args->tree) {
            output.tree(rows, commands.current_id());
        } else {
            output.table(rows, commands.current_id());
        }
    });
    command->alias("ls");
    command->add_flag("--tree,-t", args->tree, "Draw workspace ancestry");
    command->add_flag("--all", args->all, "List every family, not just the current one");
    command->add_flag("--trash", args->trash, "Include trashed workspaces");
}

void add_path(Commands& commands) {
    auto target = std::make_shared<std::string>();
    auto* command = commands.add("path", "Print a workspace's path", [&commands, target] {
        print_workspace(commands, commands.workspaces().resolve(*target));
    });
    command->alias("cd");
    command->add_option("target", *target, "Handle, ID, path, `root` (default: current)");
}

void add_pwd(Commands& commands) {
    commands.add("pwd", "Print the current workspace's path",
                 [&commands] { print_workspace(commands, commands.workspaces().current()); });
}

void add_ancestors(Commands& commands) {
    auto target = std::make_shared<std::string>();
    auto* command = commands.add("ancestors", "List a workspace's ancestors", [&commands, target] {
        auto workspace = commands.workspaces().resolve(*target);
        auto chain = commands.workspaces().ancestors(workspace);
        const auto output = commands.output();
        if (output.json()) {
            output.emit({{"workspaces", to_json(chain)}});
        } else {
            output.table(chain, std::nullopt);
        }
    });
    command->add_option("target", *target, "Handle, ID, or path (default: current)");
}

// Development aid for measuring the walker; not part of the public interface.
void add_copy_tree(Commands& commands) {
    struct Args {
        std::string from;
        std::string to;
        bool copy = false;
        bool filtered = false;
    };
    auto args = std::make_shared<Args>();
    auto* command = commands.add("_copy-tree", "Copy a directory tree", [args] {
        CopyTreeOptions options;
        options.mode = args->copy ? CopyMode::copy : CopyMode::cow;
        if (args->filtered) {
            options.skip = filter_excludes;
        }
        copy_tree(args->from, args->to, options);
    });
    command->group("");
    command->add_option("from", args->from)->required();
    command->add_option("to", args->to)->required();
    command->add_flag("--copy", args->copy);
    command->add_flag("--filtered", args->filtered);
}

int run(int argc, char** argv) {
    CLI::App app{"Copy-on-write workspaces for parallel humans and agents", "hz"};
    app.set_version_flag("--version,-V", std::string("hz ") + hz::version);
    app.require_subcommand(0, 1);

    Commands commands(app);
    add_init(commands);
    add_list(commands);
    add_path(commands);
    add_pwd(commands);
    add_ancestors(commands);
    add_copy_tree(commands);

    try {
        app.parse(argc, argv);
    } catch (const CLI::ParseError& error) {
        return app.exit(error);
    }
    return commands.run();
}

} // namespace

} // namespace hz::cli

int main(int argc, char** argv) {
    try {
        return hz::cli::run(argc, argv);
    } catch (const std::exception& error) {
        // iostreams do not throw by default, so the handler itself is exception-free.
        std::cerr << "hz: " << error.what() << '\n';
        return 1;
    }
}
