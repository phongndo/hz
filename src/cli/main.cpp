#include "hz/version.hpp"

#include <CLI/CLI.hpp>

#include <exception>
#include <iostream>
#include <print>
#include <string>

namespace {

int run(int argc, char** argv) {
    CLI::App app{"Copy-on-write workspaces for parallel humans and agents", "hz"};
    app.set_version_flag("--version,-V", std::string("hz ") + hz::version);
    app.require_subcommand(0, 1);

    CLI11_PARSE(app, argc, argv);

    if (app.get_subcommands().empty()) {
        std::print("{}", app.help());
    }
    return 0;
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
