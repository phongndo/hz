#pragma once

#include "hz/workspaces.hpp"

#include <CLI/CLI.hpp>

#include <functional>
#include <map>
#include <optional>
#include <string>

#include "cli/output.hpp"

namespace hz::cli {

// Registers subcommands and remembers which action belongs to which, so the
// chosen action runs once parsing (including trailing global flags) is done.
class Commands {
  public:
    explicit Commands(CLI::App& app);

    // Adds a subcommand of `parent` (the top level when null).
    CLI::App* add(const std::string& name, const std::string& description,
                  std::function<void()> action, CLI::App* parent = nullptr);
    // Adds a command group such as `git` whose subcommands carry the actions.
    CLI::App* group(const std::string& name, const std::string& description);

    // Runs the parsed command; returns the process exit code.
    int run();

    [[nodiscard]] Output output() const { return Output(json_ || machine_); }

    // Opened lazily so that --help and --version never touch the registry.
    Workspaces& workspaces();
    std::optional<std::string> current_id();

    // Makes the command exit unsuccessfully after printing its normal output,
    // as `hz doctor` does when problems remain.
    void set_exit_code(int code) { exit_code_ = code; }

  private:
    int exit_code_ = 0;
    CLI::App& app_;
    bool json_ = false;
    bool machine_ = false;
    std::string at_;
    std::map<CLI::App*, std::function<void()>> actions_;
    std::optional<Workspaces> workspaces_;
};

void add_workspace_commands(Commands& commands);
void add_integration_commands(Commands& commands);

} // namespace hz::cli
