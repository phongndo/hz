#include "hz/version.hpp"

#include <CLI/CLI.hpp>

#include <exception>
#include <iostream>
#include <string>

#include "cli/commands.hpp"

namespace {

int run(int argc, char** argv) {
    CLI::App app{"Copy-on-write workspaces for parallel humans and agents", "hz"};
    app.set_version_flag("--version,-V", std::string("hz ") + hz::version);
    app.require_subcommand(0, 1);

    hz::cli::Commands commands(app);
    hz::cli::add_workspace_commands(commands);
    hz::cli::add_integration_commands(commands);

    try {
        app.parse(argc, argv);
    } catch (const CLI::ParseError& error) {
        return app.exit(error);
    }
    return commands.run();
}

} // namespace

int main(int argc, char** argv) {
    try {
        return run(argc, argv);
    } catch (const std::exception& error) {
        // iostreams do not throw by default, so the handler itself is exception-free.
        std::cerr << "hz: " << error.what() << '\n';
        return 1;
    }
}
