#include "cli/commands.hpp"

#include "hz/error.hpp"

#include <filesystem>
#include <print>

namespace hz::cli {

namespace fs = std::filesystem;

Commands::Commands(CLI::App& app) : app_(app) {
    app_.add_flag("--json,-j", json_, "Print JSON");
    app_.add_flag("--machine", machine_, "Print JSON; shell integration never changes directory");
    app_.add_option("--at,-a", at_, "Resolve the current workspace from DIR")->type_name("DIR");
}

CLI::App* Commands::add(const std::string& name, const std::string& description,
                        std::function<void()> action, CLI::App* parent) {
    auto* command = (parent != nullptr ? parent : &app_)->add_subcommand(name, description);
    command->fallthrough();
    actions_[command] = std::move(action);
    return command;
}

CLI::App* Commands::group(const std::string& name, const std::string& description) {
    auto* command = app_.add_subcommand(name, description);
    command->fallthrough();
    command->require_subcommand(1);
    return command;
}

int Commands::run() {
    const Output out = output();
    for (auto& [command, action] : actions_) {
        if (command->parsed()) {
            try {
                action();
                return exit_code_;
            } catch (const Error& error) {
                out.error(error);
                return 1;
            } catch (const fs::filesystem_error& error) {
                out.error(Error(ErrorKind::io, error.what(), error.code()));
                return 1;
            }
        }
    }
    std::print("{}", app_.help());
    return 0;
}

Workspaces& Commands::workspaces() {
    if (!workspaces_) {
        const fs::path context = at_.empty() ? fs::current_path() : fs::absolute(at_);
        workspaces_.emplace(default_data_directory(), context);
    }
    return *workspaces_;
}

std::optional<std::string> Commands::current_id() {
    try {
        if (auto here = workspaces().current_optional()) {
            return here->id;
        }
    } catch (const Error&) {
        return std::nullopt; // an inconsistent context has no current workspace
    }
    return std::nullopt;
}

} // namespace hz::cli
