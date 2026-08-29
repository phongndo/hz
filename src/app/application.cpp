#include "app/application.hpp"

#include "light/version.hpp"

#include <cstddef>
#include <iostream>
#include <ostream>
#include <span>
#include <string_view>
#include <vector>

namespace light::app {
namespace {

constexpr std::string_view help_text = R"(light creates fast, independent development workspaces.

Usage: light [OPTIONS]

Options:
  -h, --help       Show this help
  -V, --version    Show the light version
)";

constexpr int usage_error = 2;

} // namespace

auto run(const std::span<const std::string_view> arguments, std::ostream& output,
         std::ostream& error) -> int {
  if (arguments.empty() ||
      (arguments.size() == 1U && (arguments.front() == "--help" || arguments.front() == "-h"))) {
    output << help_text;
    return 0;
  }

  if (arguments.size() == 1U && (arguments.front() == "--version" || arguments.front() == "-V")) {
    output << "light " << light::version << '\n';
    return 0;
  }

  error << "light: unsupported argument:";
  for (const auto argument : arguments) {
    error << ' ' << argument;
  }
  error << "\nRun 'light --help' for usage.\n";
  return usage_error;
}

auto run(const int argument_count, char** arguments) -> int {
  std::vector<std::string_view> argument_views;
  if (argument_count > 1) {
    const auto all_arguments =
        std::span<char*>{arguments, static_cast<std::size_t>(argument_count)};
    argument_views.reserve(all_arguments.size() - 1U);
    for (const auto* const argument : all_arguments.subspan(1U)) {
      argument_views.emplace_back(argument);
    }
  }

  return run(argument_views, std::cout, std::cerr);
}

} // namespace light::app
