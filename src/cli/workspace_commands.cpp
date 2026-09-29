#include <filesystem>
#include <format>
#include <memory>
#include <string>
#include <vector>

#include "cli/commands.hpp"

namespace hz::cli {

namespace fs = std::filesystem;

namespace {

std::string handles(const std::vector<Workspace>& workspaces) {
    std::string text;
    for (const auto& workspace : workspaces) {
        text += (text.empty() ? "" : ", ") + workspace.handle;
    }
    return text;
}

// Parses repeated `--label KEY=VALUE` arguments; a key may appear once.
Labels parse_labels(const std::vector<std::string>& arguments) {
    Labels labels;
    for (const auto& argument : arguments) {
        auto [key, value] = parse_label(argument);
        if (labels.contains(key)) {
            throw Error(ErrorKind::invalid_argument,
                        std::format("label '{}' is given more than once", key));
        }
        labels.emplace(std::move(key), std::move(value));
    }
    return labels;
}

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
            const fs::path target =
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
                      "Use byte copies where the filesystem cannot clone (slower, uses space)");
    command->add_flag("--path-only", args->path_only, "Print only the workspace path");
}

void add_new(Commands& commands) {
    struct Args {
        std::string name;
        std::string from;
        std::string into;
        bool filtered = false;
        bool full = false;
        bool no_hooks = false;
        bool path_only = false;
        std::vector<std::string> labels;
    };
    auto args = std::make_shared<Args>();
    auto* command = commands.add("new", "Create a child workspace", [&commands, args] {
        CreateOptions options;
        options.labels = parse_labels(args->labels);
        options.source = args->from;
        if (!args->name.empty()) {
            options.handle = args->name;
        }
        if (!args->into.empty()) {
            options.into = fs::path(args->into);
        }
        if (args->filtered || args->full) {
            options.filtered = args->filtered;
        }
        options.hooks = !args->no_hooks;
        auto workspace = commands.workspaces().create(options);
        const auto output = commands.output();
        if (output.json()) {
            output.emit({{"workspace", to_json(workspace)}});
        } else if (args->path_only) {
            output.line(workspace.path.string());
        } else {
            output.line(std::format("Created {} at {}", workspace.handle, workspace.path.string()));
        }
    });
    command->add_option("name", args->name, "Handle for the new workspace (default: generated)");
    command->add_option("--from", args->from, "Workspace to copy (default: current)");
    command->add_option("--into", args->into, "Store the workspace in DIR, on the same filesystem")
        ->type_name("DIR");
    auto* filtered = command->add_flag("--filtered", args->filtered,
                                       "Omit regenerable artifacts such as node_modules");
    command->add_flag("--full", args->full, "Copy everything, overriding [create] filtered")
        ->excludes(filtered);
    command->add_flag("--no-hooks", args->no_hooks, "Skip postcreate hooks");
    command->add_flag("--path-only", args->path_only, "Print only the new workspace's path");
    command->add_option("--label,-l", args->labels, "Attach a label; repeatable")
        ->type_name("KEY=VALUE")
        ->allow_extra_args(false);
}

void add_remove(Commands& commands) {
    struct Args {
        std::string target;
        bool children = false;
        bool force = false;
        bool no_hooks = false;
        bool path_only = false;
    };
    auto args = std::make_shared<Args>();
    auto* command =
        commands.add("remove", "Move a workspace and its descendants to trash", [&commands, args] {
            auto result = commands.workspaces().remove({.target = args->target,
                                                        .children_only = args->children,
                                                        .force = args->force,
                                                        .hooks = !args->no_hooks});
            const auto output = commands.output();
            if (output.json()) {
                nlohmann::json document{{"trashed", to_json(result.trashed)},
                                        {"unregistered", nullptr},
                                        {"navigate_to", nullptr}};
                if (result.unregistered) {
                    document["unregistered"] = to_json(*result.unregistered);
                }
                if (result.navigate_to) {
                    document["navigate_to"] = result.navigate_to->string();
                }
                output.emit(document);
            } else if (args->path_only) {
                if (result.navigate_to) {
                    output.line(result.navigate_to->string());
                }
            } else {
                if (result.unregistered) {
                    output.line(std::format("Unregistered root {} (its directory is kept)",
                                            result.unregistered->handle));
                }
                if (result.trashed.size() == 1) {
                    output.line(std::format("Moved {} to trash; `hz restore {}` brings it back",
                                            result.trashed.front().handle,
                                            result.trashed.front().handle));
                } else if (!result.trashed.empty()) {
                    output.line(std::format("Moved {} workspaces to trash: {}",
                                            result.trashed.size(), handles(result.trashed)));
                } else if (!result.unregistered) {
                    output.line("Nothing to remove");
                }
            }
        });
    command->alias("rm");
    command->add_option("target", args->target, "Handle, ID, or path (default: current)");
    command->add_flag("--children", args->children, "Keep the target; remove its descendants");
    command->add_flag("--force,-f", args->force, "Allow unregistering a root");
    command->add_flag("--no-hooks", args->no_hooks, "Skip preremove hooks");
    command->add_flag("--path-only", args->path_only,
                      "Print only where the shell should go if it was inside");
}

void add_restore(Commands& commands) {
    struct Args {
        std::string target;
        bool path_only = false;
    };
    auto args = std::make_shared<Args>();
    auto* command = commands.add("restore", "Bring a trashed workspace back with its descendants",
                                 [&commands, args] {
                                     auto restored = commands.workspaces().restore(args->target);
                                     const auto output = commands.output();
                                     if (output.json()) {
                                         output.emit({{"restored", to_json(restored)}});
                                     } else if (args->path_only) {
                                         output.line(restored.front().path.string());
                                     } else {
                                         output.line(std::format("Restored {}", handles(restored)));
                                     }
                                 });
    command->add_option("target", args->target, "Trashed handle or ID")->required();
    command->add_flag("--path-only", args->path_only, "Print only the restored path");
}

void add_gc(Commands& commands) {
    commands.add("gc", "Permanently delete everything in trash", [&commands] {
        auto result = commands.workspaces().gc();
        const auto output = commands.output();
        if (output.json()) {
            output.emit({{"deleted", to_json(result.deleted)}});
        } else if (result.deleted.empty()) {
            output.line("Trash is empty");
        } else {
            output.line(std::format("Deleted {} workspace{} from trash", result.deleted.size(),
                                    result.deleted.size() == 1 ? "" : "s"));
        }
    });
}

void add_pin(Commands& commands, bool pin) {
    auto targets = std::make_shared<std::vector<std::string>>();
    auto* command = commands.add(
        pin ? "pin" : "unpin",
        pin ? "Protect workspaces from removal" : "Allow removing workspaces again",
        [&commands, targets, pin] {
            std::vector<Workspace> changed;
            for (const auto& target : *targets) {
                changed.push_back(commands.workspaces().set_pinned(target, pin));
            }
            const auto output = commands.output();
            if (output.json()) {
                output.emit({{"workspaces", to_json(changed)}});
            } else {
                output.line(std::format("{} {}", pin ? "Pinned" : "Unpinned", handles(changed)));
            }
        });
    command->add_option("targets", *targets, "Handles, IDs, or paths")->required();
}

void add_label(Commands& commands) {
    struct Args {
        std::string target;
        std::vector<std::string> labels;
        std::vector<std::string> unset;
    };
    auto args = std::make_shared<Args>();
    auto* command = commands.add("label", "Show or change a workspace's labels", [&commands, args] {
        const auto set = parse_labels(args->labels);
        auto workspace = set.empty() && args->unset.empty()
                             ? commands.workspaces().resolve(args->target)
                             : commands.workspaces().set_labels(args->target, set, args->unset);
        const auto output = commands.output();
        if (output.json()) {
            output.emit({{"workspace", to_json(workspace)}});
        } else {
            for (const auto& [key, value] : workspace.labels) {
                output.line(std::format("{}={}", key, value));
            }
        }
    });
    command->add_option("target", args->target, "Handle, ID, or path")->required();
    command->add_option("labels", args->labels, "Labels to set")->type_name("KEY=VALUE");
    command->add_option("--unset,-u", args->unset, "Remove a label; repeatable")
        ->type_name("KEY")
        ->allow_extra_args(false);
}

void add_adopt(Commands& commands) {
    auto path = std::make_shared<std::string>();
    auto* command = commands.add(
        "adopt", "Record that a workspace directory was moved here", [&commands, path] {
            auto workspace = commands.workspaces().adopt(*path);
            const auto output = commands.output();
            if (output.json()) {
                output.emit({{"workspace", to_json(workspace)}});
            } else {
                output.line(
                    std::format("Adopted {} at {}", workspace.handle, workspace.path.string()));
            }
        });
    command->add_option("path", *path, "The workspace's new location")->required();
}

void add_doctor(Commands& commands) {
    auto fix = std::make_shared<bool>(false);
    auto* command =
        commands.add("doctor", "Check the registry against the filesystem", [&commands, fix] {
            auto findings = commands.workspaces().doctor(*fix);
            const bool unresolved = std::ranges::any_of(
                findings, [](const Finding& finding) { return !finding.fixed; });
            const auto output = commands.output();
            if (output.json()) {
                auto list = nlohmann::json::array();
                for (const auto& finding : findings) {
                    list.push_back({{"kind", finding.kind},
                                    {"message", finding.message},
                                    {"workspace_id", finding.workspace_id.value_or("")},
                                    {"path", finding.path ? finding.path->string() : ""},
                                    {"fixed", finding.fixed}});
                }
                output.emit({{"findings", list}, {"ok", !unresolved}});
            } else if (findings.empty()) {
                output.line("No problems found");
            } else {
                for (const auto& finding : findings) {
                    output.line(std::format("{}{}: {}", finding.fixed ? "fixed " : "", finding.kind,
                                            finding.message));
                }
                if (unresolved && !*fix) {
                    output.line("Run `hz doctor --fix` to repair what can be proven safe");
                }
            }
            if (unresolved) {
                commands.set_exit_code(exit_status(ErrorKind::inconsistent));
            }
        });
    command->add_flag("--fix", *fix, "Repair problems whose resolution is provable");
}

void add_list(Commands& commands) {
    struct Args {
        bool tree = false;
        bool all = false;
        bool trash = false;
        std::vector<std::string> labels;
    };
    auto args = std::make_shared<Args>();
    auto* command = commands.add("list", "List workspaces", [&commands, args] {
        ListOptions options{.all_families = args->all, .include_trashed = args->trash};
        for (const auto& selector : args->labels) {
            options.labels.push_back(parse_label_selector(selector));
        }
        auto rows = commands.workspaces().list(options);
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
    command
        ->add_option("--label,-l", args->labels,
                     "Only workspaces with this label, or this key; repeatable")
        ->type_name("KEY[=VALUE]")
        ->allow_extra_args(false);
}

void add_path(Commands& commands) {
    auto target = std::make_shared<std::string>();
    auto* command = commands.add("path", "Print a workspace's path", [&commands, target] {
        print_workspace(commands, commands.workspaces().resolve(*target));
    });
    command->alias("cd");
    command->add_option("target", *target, "Handle, ID, path, or `root` (default: current)");
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

} // namespace

void add_workspace_commands(Commands& commands) {
    add_init(commands);
    add_new(commands);
    add_list(commands);
    add_path(commands);
    add_pwd(commands);
    add_ancestors(commands);
    add_remove(commands);
    add_restore(commands);
    add_gc(commands);
    add_pin(commands, true);
    add_pin(commands, false);
    add_label(commands);
    add_adopt(commands);
    add_doctor(commands);
}

} // namespace hz::cli
