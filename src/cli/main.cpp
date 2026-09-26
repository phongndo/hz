#include "hz/filter.hpp"
#include "hz/tree.hpp"
#include "hz/version.hpp"

#include <CLI/CLI.hpp>

#include <exception>
#include <iostream>
#include <memory>
#include <print>
#include <string>

namespace {

// Development aid for measuring the walker; not part of the public interface.
void add_copy_tree_command(CLI::App& app) {
    struct Args {
        std::string from;
        std::string to;
        bool copy = false;
        bool filtered = false;
    };
    auto args = std::make_shared<Args>();
    auto* command = app.add_subcommand("_copy-tree", "Copy a directory tree")->group("");
    command->add_option("from", args->from)->required();
    command->add_option("to", args->to)->required();
    command->add_flag("--copy", args->copy, "Duplicate bytes instead of cloning");
    command->add_flag("--filtered", args->filtered, "Omit regenerable artifacts");
    command->callback([args] {
        hz::CopyTreeOptions options;
        options.mode = args->copy ? hz::CopyMode::copy : hz::CopyMode::cow;
        if (args->filtered) {
            options.skip = hz::filter_excludes;
        }
        hz::copy_tree(args->from, args->to, options);
    });
}

int run(int argc, char** argv) {
    CLI::App app{"Copy-on-write workspaces for parallel humans and agents", "hz"};
    app.set_version_flag("--version,-V", std::string("hz ") + hz::version);
    app.require_subcommand(0, 1);
    add_copy_tree_command(app);

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
