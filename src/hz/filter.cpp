#include "hz/filter.hpp"

#include <algorithm>
#include <array>
#include <string_view>

namespace hz {

namespace {

constexpr std::array source_control_directories{".git", ".hg", ".jj"};

constexpr std::array excluded_components{
    "node_modules", ".pnpm-store", "target",        ".venv",       "venv",          ".tox",
    ".nox",         "__pycache__", ".pytest_cache", ".mypy_cache", ".ruff_cache",   ".next",
    ".nuxt",        ".svelte-kit", ".turbo",        ".vite",       ".parcel-cache", ".cache",
    "dist",         "build",       "coverage",
};

constexpr std::array yarn_artifacts{"cache", "unplugged", "install-state.gz", "build-state.yml"};

bool contains(const auto& names, std::string_view name) {
    return std::ranges::find(names, name) != names.end();
}

} // namespace

bool filter_excludes(const std::filesystem::path& relative) {
    bool source_control = false;
    bool git_metadata = false;
    bool excluded = false;
    std::string_view previous;
    for (const auto& component : relative) {
        std::string_view part = component.native();
        if (part.empty() || part == "." || part == "/") {
            continue;
        }
        if (contains(source_control_directories, part)) {
            source_control = true;
            git_metadata = git_metadata || part == ".git";
        } else if (git_metadata && part == "fsmonitor--daemon.ipc") {
            return true;
        }
        excluded = excluded || contains(excluded_components, part) ||
                   (previous == ".yarn" && contains(yarn_artifacts, part));
        previous = part;
    }
    return !source_control && excluded;
}

} // namespace hz
