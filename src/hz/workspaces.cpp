#include "hz/workspaces.hpp"

#include "hz/error.hpp"
#include "hz/fsutil.hpp"
#include "hz/git.hpp"
#include "hz/handle.hpp"
#include "hz/marker.hpp"
#include "hz/ulid.hpp"

#include <algorithm>
#include <cctype>
#include <cstdlib>
#include <format>
#include <sys/stat.h>

namespace hz {

namespace fs = std::filesystem;

namespace {

fs::path home_directory() {
    if (const char* home = std::getenv("HOME"); home != nullptr && *home != '\0') {
        return home;
    }
    throw Error(ErrorKind::invalid_path, "HOME is not set; set HZ_DATA_DIR");
}

bool looks_like_path(std::string_view target) {
    return target.find('/') != std::string_view::npos || target == "." || target == ".." ||
           target.starts_with('~');
}

std::string upper(std::string_view text) {
    std::string result(text);
    std::ranges::transform(result, result.begin(),
                           [](unsigned char c) { return static_cast<char>(std::toupper(c)); });
    return result;
}

std::string describe(const std::vector<Workspace>& matches) {
    std::string text;
    for (const auto& workspace : matches) {
        text +=
            std::format("\n  {} {} {}", workspace.handle, workspace.id, workspace.path.string());
    }
    return text;
}

std::vector<Workspace> without_trashed(std::vector<Workspace> rows) {
    std::erase_if(rows, [](const Workspace& w) { return w.state == State::trashed; });
    return rows;
}

} // namespace

fs::path default_data_directory() {
    if (const char* dir = std::getenv("HZ_DATA_DIR"); dir != nullptr && *dir != '\0') {
        return dir;
    }
#ifdef __APPLE__
    return home_directory() / "Library" / "Application Support" / "hz";
#else
    if (const char* xdg = std::getenv("XDG_DATA_HOME"); xdg != nullptr && *xdg == '/') {
        return fs::path(xdg) / "hz";
    }
    return home_directory() / ".local" / "share" / "hz";
#endif
}

fs::path canonical_directory(const fs::path& path) {
    std::error_code error;
    fs::path canonical = fs::canonical(path, error);
    if (error) {
        throw Error(ErrorKind::invalid_path, std::format("{}: {}", path.string(), error.message()),
                    error);
    }
    if (!fs::is_directory(canonical)) {
        throw Error(ErrorKind::invalid_path, std::format("{} is not a directory", path.string()));
    }
    return canonical;
}

Workspaces::Workspaces(const fs::path& data_directory, fs::path context)
    : registry_((make_private_directories(data_directory), data_directory / "hz.sqlite")),
      context_(std::move(context)), data_directory_(data_directory) {}

detail::Fd Workspaces::operation_lock() {
    return lock_operations(data_directory_);
}

void Workspaces::require_separate_directory(const fs::path& directory, std::string_view except_id) {
    for (const auto& workspace : registry_.all()) {
        if (workspace.id == except_id) {
            continue;
        }
        // Reserve the trash directory too, including any .deleting siblings.
        // Physical nesting would let deleting one workspace erase another.
        const auto trash =
            workspace.trash_path ? workspace.trash_path->parent_path() : workspace.path;
        for (const auto& location : {workspace.path, trash}) {
            if (is_within(directory, location) || is_within(location, directory)) {
                throw Error(ErrorKind::invalid_path,
                            std::format("{} overlaps workspace '{}' at {}; workspaces must be "
                                        "separate directories",
                                        directory.string(), workspace.handle, location.string()));
            }
        }
    }
}

InitResult Workspaces::init(const fs::path& directory, CopyMode mode) {
    const auto lock = operation_lock();
    const fs::path path =
        canonical_directory(directory.is_absolute() ? directory : context_ / directory);
    if (path == path.root_path()) {
        throw Error(ErrorKind::invalid_path, "the filesystem root cannot be a workspace");
    }

    std::optional<std::string> existing_id = read_marker(path);
    if (existing_id) {
        if (auto existing = registry_.find(*existing_id)) {
            if (existing->path == path && existing->state == State::active) {
                require_quiescent(*existing);
                git::prepare_root(path);
                return {.workspace = *existing, .created = false};
            }
            throw Error(ErrorKind::inconsistent,
                        std::format("{} carries the marker of workspace '{}' registered at {}; "
                                    "if you moved it there, run `hz adopt {}`",
                                    path.string(), existing->handle, existing->path.string(),
                                    path.string()));
        }
        // A marker the registry does not know (for example after the registry
        // was deleted): explicitly initializing the directory re-registers it.
    }
    if (auto enclosing = find_marked_directory(path.parent_path())) {
        throw Error(ErrorKind::conflict,
                    std::format("{} is inside the workspace at {}; initialize a directory "
                                "outside any workspace",
                                path.string(), enclosing->string()));
    }
    if (auto registered = registry_.find_by_path(path)) {
        // Registered but its marker is missing: the caller is asserting this
        // directory is that root, so restore the marker.
        if (registered->is_root() && registered->state == State::active) {
            write_marker(path, registered->id);
            return {.workspace = *registered, .created = false};
        }
        throw Error(ErrorKind::conflict,
                    std::format("{} is registered as workspace '{}' ({})", path.string(),
                                registered->handle, to_string(registered->state)));
    }
    require_separate_directory(path);
    if (mode == CopyMode::cow && !probe_clone_support(path)) {
        throw Error(ErrorKind::cow_unavailable,
                    std::format("the filesystem at {} cannot clone files; run `hz init --copy` "
                                "to use byte copies instead",
                                path.string()));
    }

    Workspace root;
    root.id = generate_ulid();
    root.root_id = root.id;
    root.handle = handle_from_name(path.filename().string());
    root.path = path;
    root.state = State::active;
    root.mode = mode;
    root.created_at = root.updated_at = now_ms();
    registry_.transaction([&] { registry_.insert(root); });
    try {
        write_marker(path, root.id);
        git::prepare_root(path);
    } catch (...) {
        registry_.transaction([&] { registry_.erase(root.id); });
        throw;
    }
    return {.workspace = root, .created = true};
}

Workspace Workspaces::require_registered(const std::string& id) {
    auto workspace = registry_.find(id);
    if (!workspace) {
        throw Error(ErrorKind::not_found, std::format("workspace {} is not registered", id));
    }
    return *workspace;
}

Workspace Workspaces::workspace_at(const fs::path& directory) {
    const auto marker = read_marker(directory);
    if (!marker) {
        throw Error(ErrorKind::not_found,
                    std::format("{} is not a workspace directory", directory.string()));
    }
    const std::string& id = *marker;
    auto workspace = registry_.find(id);
    if (!workspace) {
        throw Error(ErrorKind::inconsistent,
                    std::format("{} carries workspace marker {}, which is not registered; run "
                                "`hz init {}` to register it as a new root",
                                directory.string(), id, directory.string()));
    }
    if (workspace->path != directory || workspace->state == State::trashed) {
        throw Error(ErrorKind::inconsistent,
                    std::format("{} carries the marker of workspace '{}' registered at {}; if "
                                "you moved it there, run `hz adopt {}`",
                                directory.string(), workspace->handle, workspace->path.string(),
                                directory.string()));
    }
    return *workspace;
}

std::optional<Workspace> Workspaces::current_optional() {
    std::error_code error;
    fs::path start = fs::canonical(context_, error);
    if (error) {
        return std::nullopt;
    }
    auto directory = find_marked_directory(start);
    if (!directory) {
        return std::nullopt;
    }
    return workspace_at(*directory);
}

Workspace Workspaces::current() {
    auto workspace = current_optional();
    if (!workspace) {
        throw Error(ErrorKind::not_found,
                    std::format("{} is not inside an hz workspace", context_.string()));
    }
    return *workspace;
}

Workspace Workspaces::resolve(std::string_view target) {
    if (target.empty()) {
        return current();
    }
    if (target == "root" || target == "local") {
        auto root = require_registered(current().root_id);
        return root;
    }
    if (looks_like_path(target)) {
        fs::path path(target);
        if (target.starts_with('~')) {
            path = home_directory() / fs::path(target.substr(target.starts_with("~/") ? 2 : 1));
        } else if (path.is_relative()) {
            path = context_ / path;
        }
        const fs::path canonical = canonical_directory(path);
        auto directory = find_marked_directory(canonical);
        if (!directory) {
            throw Error(ErrorKind::not_found,
                        std::format("{} is not inside an hz workspace", canonical.string()));
        }
        return workspace_at(*directory);
    }

    std::optional<Workspace> here;
    try {
        here = current_optional();
    } catch (const Error&) {
        here.reset(); // an inconsistent context should not prevent resolving by name
    }
    if (here) {
        if (auto match = registry_.find_handle(here->root_id, target)) {
            return *match;
        }
    }
    const std::string id = upper(target);
    if (auto match = registry_.find(id); match && match->state != State::trashed) {
        return *match;
    }
    auto by_handle = registry_.find_handle_anywhere(target);
    if (by_handle.size() == 1) {
        return by_handle.front();
    }
    if (by_handle.size() > 1) {
        throw Error(ErrorKind::ambiguous,
                    std::format("'{}' names workspaces in several families; use an ID or a "
                                "path:{}",
                                target, describe(by_handle)));
    }
    auto by_prefix = without_trashed(registry_.find_id_prefix(id));
    if (by_prefix.size() == 1) {
        return by_prefix.front();
    }
    if (by_prefix.size() > 1) {
        throw Error(ErrorKind::ambiguous, std::format("'{}' matches several workspace IDs:{}",
                                                      target, describe(by_prefix)));
    }
    throw Error(ErrorKind::not_found, std::format("no workspace matches '{}'", target));
}

std::vector<Workspace> Workspaces::list(const ListOptions& options) {
    std::vector<Workspace> rows;
    std::optional<Workspace> here;
    if (!options.all_families) {
        here = current_optional();
    }
    rows = here ? registry_.family(here->root_id) : registry_.all();
    if (!options.include_trashed) {
        rows = without_trashed(std::move(rows));
    }
    return rows;
}

std::vector<Workspace> Workspaces::ancestors(const Workspace& workspace) {
    std::vector<Workspace> chain;
    std::optional<std::string> parent = workspace.parent_id;
    while (parent) {
        auto row = registry_.find(*parent);
        if (!row) {
            throw Error(
                ErrorKind::inconsistent,
                std::format("workspace {} refers to missing parent {}", workspace.id, *parent));
        }
        parent = row->parent_id;
        chain.push_back(std::move(*row));
    }
    std::ranges::reverse(chain);
    return chain;
}

} // namespace hz
