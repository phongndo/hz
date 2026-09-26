// Workspace lifecycle: create, remove, restore, gc, pin, adopt, and doctor.
//
// Each mutating operation records its intent in the registry before touching
// the filesystem and reverts both if the filesystem step fails, so that any
// interruption leaves a state `hz doctor --fix` can finish or undo.

#include "hz/config.hpp"
#include "hz/error.hpp"
#include "hz/filter.hpp"
#include "hz/fsutil.hpp"
#include "hz/git.hpp"
#include "hz/handle.hpp"
#include "hz/marker.hpp"
#include "hz/process.hpp"
#include "hz/tree.hpp"
#include "hz/ulid.hpp"
#include "hz/workspaces.hpp"

#include <algorithm>
#include <cctype>
#include <format>
#include <ranges>
#include <set>
#include <unistd.h>

namespace hz {

namespace fs = std::filesystem;

namespace {

constexpr std::string_view storage_name = ".hz-workspaces";
constexpr std::string_view trash_name = ".trash";
constexpr std::string_view deleting_suffix = ".deleting";

std::string upper(std::string_view text) {
    std::string result(text);
    std::ranges::transform(result, result.begin(),
                           [](unsigned char c) { return static_cast<char>(std::toupper(c)); });
    return result;
}

fs::path trash_location(const Workspace& workspace) {
    return workspace.path.parent_path() / trash_name / workspace.id;
}

fs::path deleting_location(const fs::path& trash_path) {
    return {trash_path.string() + std::string(deleting_suffix)};
}

// The marker ID in `directory`, or nothing if it has none or it is unreadable.
std::optional<std::string> marker_or_nothing(const fs::path& directory) {
    try {
        return read_marker(directory);
    } catch (const Error&) {
        return std::nullopt;
    }
}

bool carries_marker(const fs::path& directory, const std::string& id) {
    return marker_or_nothing(directory) == id;
}

// Never move or delete a directory unless it proves it is the workspace we
// think it is.
void require_marker(const fs::path& directory, const Workspace& workspace) {
    if (!carries_marker(directory, workspace.id)) {
        throw Error(ErrorKind::inconsistent,
                    std::format("{} does not carry the marker of workspace '{}'; refusing to "
                                "touch it (run `hz doctor`)",
                                directory.string(), workspace.handle));
    }
}

bool skip_for_copy(const fs::path& relative, bool filtered) {
    const auto name = relative.filename();
    // Markers and storage directories of nested workspaces never propagate.
    if (name == marker_name || name == storage_name) {
        return true;
    }
    const bool ephemeral =
        name == "fsmonitor--daemon.ipc" && std::ranges::find(relative, ".git") != relative.end();
    return ephemeral || (filtered && filter_excludes(relative));
}

// GC keeps the marker until the last unlink. An empty directory without a
// marker can be the final interrupted rmdir; anything else must prove identity.
bool safe_to_finish_gc(const fs::path& directory, const Workspace& workspace) {
    if (carries_marker(directory, workspace.id)) {
        return true;
    }
    std::error_code error;
    return !exists_nofollow(directory / marker_name) &&
           fs::is_directory(fs::symlink_status(directory, error)) && !error &&
           fs::is_empty(directory, error) && !error;
}

void finish_gc(const fs::path& directory, const Workspace& workspace) {
    if (exists_nofollow(directory)) {
        if (!safe_to_finish_gc(directory, workspace)) {
            require_marker(directory, workspace);
        }
        remove_tree(directory);
    }
}

void try_rmdir(const fs::path& directory) {
    ::rmdir(directory.c_str());
}

// Runs lifecycle hooks in `workspace`, stopping at the first failure.
void run_hooks(const std::vector<Command>& commands, std::string_view lifecycle,
               const Workspace& workspace, const fs::path& source, const fs::path& root) {
    for (Command argv : commands) {
        if (argv.front().find('/') != std::string::npos && fs::path(argv.front()).is_relative()) {
            argv.front() = (workspace.path / argv.front()).string();
        }
        const ProcessOptions options{.cwd = workspace.path,
                                     .input = {},
                                     .env = {{"HZ_ROOT", root.string()},
                                             {"HZ_SOURCE", source.string()},
                                             {"HZ_WORKSPACE", workspace.path.string()},
                                             {"HZ_WORKSPACE_ID", workspace.id},
                                             {"HZ_PARENT_ID", workspace.parent_id.value_or("")},
                                             {"HZ_HANDLE", workspace.handle},
                                             {"HZ_LIFECYCLE", std::string(lifecycle)}},
                                     .passthrough = true};
        ProcessResult result;
        try {
            result = run_process(argv, options);
        } catch (const Error& error) {
            throw Error(ErrorKind::hook_failed, std::format("{} hook in {}: {}", lifecycle,
                                                            workspace.path.string(), error.what()));
        }
        if (!result.ok()) {
            throw Error(ErrorKind::hook_failed,
                        std::format("{} hook `{}` exited with status {} in {}", lifecycle,
                                    argv.front(), result.exit_code, workspace.path.string()));
        }
    }
}

void register_child(Registry& registry, Workspace& child,
                    const std::optional<std::string>& handle) {
    if (handle) {
        require_valid_handle(*handle);
        child.handle = *handle;
        registry.transaction([&] { registry.insert(child); });
    } else {
        // Two processes can draw the same free name; retry on the collision.
        for (int attempt = 0;; ++attempt) {
            child.handle = generate_handle([&](std::string_view candidate) {
                return registry.find_handle(child.root_id, candidate).has_value();
            });
            try {
                registry.transaction([&] { registry.insert(child); });
                break;
            } catch (const Error& error) {
                if (error.kind() != ErrorKind::conflict || attempt == 4) {
                    throw;
                }
            }
        }
    }
}

void check_removable(const Workspace& workspace) {
    if (workspace.pinned) {
        throw Error(ErrorKind::conflict, std::format("'{}' is pinned; run `hz unpin {}` first",
                                                     workspace.handle, workspace.handle));
    }
    if (workspace.state == State::creating && workspace.pid && process_alive(*workspace.pid)) {
        throw Error(ErrorKind::conflict,
                    std::format("'{}' is still being created", workspace.handle));
    }
}

void run_preremove_hooks(Registry& registry, const std::vector<Workspace>& batch) {
    for (const auto& workspace : batch) {
        if (!exists_nofollow(workspace.path)) {
            continue;
        }
        const auto parent =
            workspace.parent_id ? registry.find(*workspace.parent_id) : std::nullopt;
        const auto root = registry.find(workspace.root_id);
        run_hooks(load_config(workspace.path).preremove, "preremove", workspace,
                  parent ? parent->path : workspace.path, root ? root->path : workspace.path);
    }
}

void check_restore(Registry& registry, const std::vector<Workspace>& batch) {
    for (const auto& workspace : batch) {
        if (!workspace.trash_path || !exists_nofollow(*workspace.trash_path)) {
            throw Error(ErrorKind::inconsistent,
                        std::format("the trashed directory of '{}' is gone; run `hz gc`",
                                    workspace.handle));
        }
        require_marker(*workspace.trash_path, workspace);
        if (exists_nofollow(workspace.path)) {
            throw Error(ErrorKind::conflict,
                        std::format("{} already exists; cannot restore '{}'",
                                    workspace.path.string(), workspace.handle));
        }
        if (auto holder = registry.find_handle(workspace.root_id, workspace.handle)) {
            throw Error(
                ErrorKind::conflict,
                std::format("the name '{}' is now used by another workspace", workspace.handle));
        }
    }
}

std::optional<fs::path> removal_destination(Registry& registry, const fs::path& context,
                                            const Workspace& target,
                                            const std::vector<Workspace>& batch, bool keep_target) {
    std::error_code error;
    const fs::path here = fs::canonical(context, error);
    if (!error) {
        const bool inside = std::ranges::any_of(
            batch, [&](const Workspace& workspace) { return is_within(here, workspace.path); });
        if (inside) {
            if (keep_target) {
                return target.path;
            }
            if (target.parent_id) {
                if (auto parent = registry.find(*target.parent_id)) {
                    return parent->path;
                }
            }
        }
    }
    return std::nullopt;
}

void check_restore_parent(Registry& registry, const Workspace& top) {
    if (top.parent_id) {
        auto parent = registry.find(*top.parent_id);
        if (!parent) {
            throw Error(ErrorKind::conflict,
                        std::format("the parent of '{}' no longer exists", top.handle));
        }
        if (parent->state != State::active) {
            throw Error(ErrorKind::conflict,
                        std::format("'{}' was removed with its parent '{}'; restore '{}' instead",
                                    top.handle, parent->handle, parent->handle));
        }
    }
}

void rollback_restore(const std::vector<const Workspace*>& moved) {
    for (const auto* workspace : moved | std::views::reverse) {
        try {
            if (workspace->trash_path) {
                move_path(workspace->path, *workspace->trash_path);
            }
        } catch (const Error&) {
            // `hz doctor --fix` finishes from the recorded state.
            continue;
        }
    }
}

} // namespace

fs::path Workspaces::storage_directory(const Workspace& root) {
    return root.path.parent_path() / storage_name /
           std::format("{}-{}", root.handle, root.id.substr(root.id.size() - 6));
}

Workspace Workspaces::create(const CreateOptions& options) {
    const auto lock = operation_lock();
    const Workspace source = resolve(options.source);
    if (source.state != State::active) {
        throw Error(ErrorKind::conflict,
                    std::format("workspace '{}' is still being created", source.handle));
    }
    require_marker(source.path, source);
    const auto root = registry_.find(source.root_id);
    if (!root) {
        throw Error(
            ErrorKind::inconsistent,
            std::format("the root of workspace '{}' is no longer registered", source.handle));
    }
    fs::path storage = storage_directory(*root);
    if (options.into) {
        storage = options.into->is_absolute() ? *options.into : context_ / *options.into;
    }
    make_private_directories(storage);
    storage = canonical_directory(storage);
    if (is_within(storage, source.path)) {
        throw Error(ErrorKind::invalid_path,
                    std::format("{} is inside the workspace being copied", storage.string()));
    }
    if (!same_filesystem(source.path, storage)) {
        throw Error(ErrorKind::invalid_path,
                    std::format("{} is on a different filesystem from {}; copy-on-write "
                                "workspaces need both on one filesystem",
                                storage.string(), source.path.string()));
    }
    git::check_source(source.path);
    const bool filtered =
        options.filtered.value_or(load_config(source.path).filtered.value_or(false));

    Workspace child;
    child.id = generate_ulid();
    child.root_id = root->id;
    child.parent_id = source.id;
    child.path = storage / child.id;
    require_separate_directory(child.path);
    child.state = State::creating;
    child.mode = root->mode;
    child.filtered = filtered;
    child.pid = ::getpid();
    child.created_at = child.updated_at = now_ms();
    register_child(registry_, child, options.handle);

    try {
        CopyTreeOptions copy;
        copy.mode = child.mode;
        copy.skip = [filtered](const fs::path& relative) {
            return skip_for_copy(relative, filtered);
        };
        copy_tree(source.path, child.path, copy);
        write_marker(child.path, child.id);
        git::prepare_child(child.path);
    } catch (...) {
        try {
            remove_tree(child.path);
            registry_.transaction([&] { registry_.erase(child.id); });
        } catch (const Error&) { // NOLINT(bugprone-empty-catch): preserve the copy error; doctor
                                 // owns cleanup.
            // Leave the row for `hz doctor --fix`; report the original failure.
        }
        throw;
    }

    child.state = State::active;
    child.pid.reset();
    child.updated_at = now_ms();
    registry_.transaction([&] { registry_.update(child); });

    if (options.hooks) {
        try {
            run_hooks(load_config(child.path).postcreate, "postcreate", child, source.path,
                      root->path);
        } catch (const Error& error) {
            throw Error(error.kind(), std::format("{}; workspace '{}' was created at {}",
                                                  error.what(), child.handle, child.path.string()));
        }
    }
    return child;
}

void Workspaces::trash(std::vector<Workspace>& batch) {
    std::vector<Workspace> original = batch;
    const std::string removal_id = generate_ulid();
    for (auto& workspace : batch) {
        if (exists_nofollow(workspace.path)) {
            require_marker(workspace.path, workspace);
            workspace.trash_path = trash_location(workspace);
        } else {
            // Nothing on disk to protect: the row is simply forgotten by gc.
            workspace.trash_path.reset();
        }
        workspace.state = State::trashed;
        workspace.removal_id = removal_id;
        workspace.pid.reset();
        workspace.updated_at = now_ms();
    }
    registry_.transaction([&] {
        for (const auto& workspace : batch) {
            registry_.update(workspace);
        }
    });

    std::vector<const Workspace*> moved;
    try {
        for (const auto& workspace : batch) {
            if (workspace.trash_path) {
                make_private_directories(workspace.trash_path->parent_path());
                move_path(workspace.path, *workspace.trash_path);
                moved.push_back(&workspace);
            }
        }
    } catch (...) {
        for (const auto* workspace : moved | std::views::reverse) {
            try {
                move_path(workspace->trash_path.value(), workspace->path);
            } catch (const Error&) {
                // Retain the location if rollback fails: active + trash_path
                // tells doctor to restore it instead of losing the directory.
                auto row = std::ranges::find(original, workspace->id, &Workspace::id);
                row->trash_path = workspace->trash_path;
            }
        }
        registry_.transaction([&] {
            for (const auto& workspace : original) {
                registry_.update(workspace);
            }
        });
        throw;
    }
}

RemoveResult Workspaces::remove(const RemoveOptions& options) {
    const auto lock = operation_lock();
    const Workspace target = resolve(options.target);
    const bool unregister = target.is_root() && !options.children_only;
    if (unregister && !options.force) {
        throw Error(ErrorKind::conflict,
                    std::format("'{}' is a root; use --force to unregister it (its directory is "
                                "kept) and move its descendants to trash",
                                target.handle));
    }

    std::vector<Workspace> batch;
    for (auto& workspace : registry_.subtree(target.id)) {
        if (workspace.state == State::trashed) {
            continue; // already removed on its own
        }
        if (workspace.id == target.id && (options.children_only || unregister)) {
            continue;
        }
        batch.push_back(std::move(workspace));
    }
    std::ranges::for_each(batch, check_removable);
    if (unregister) {
        check_removable(target);
        if (exists_nofollow(target.path / marker_name)) {
            require_marker(target.path, target);
        }
    }

    RemoveResult result;
    result.navigate_to = removal_destination(registry_, context_, target, batch,
                                             options.children_only || unregister);

    if (options.hooks) {
        run_preremove_hooks(registry_, batch);
    }

    trash(batch);
    result.trashed = batch;
    if (unregister) {
        remove_marker(target.path);
        registry_.transaction([&] { registry_.erase(target.id); });
        result.unregistered = target;
    }
    return result;
}

Workspace Workspaces::resolve_trashed(std::string_view target) {
    if (target.empty()) {
        throw Error(ErrorKind::invalid_argument, "name the trashed workspace to restore");
    }
    std::optional<Workspace> here;
    try {
        here = current_optional();
    } catch (const Error&) {
        here.reset();
    }
    if (here) {
        auto matches = registry_.find_trashed_handle(here->root_id, target);
        if (!matches.empty()) {
            return matches.front(); // the most recently removed
        }
    }
    const std::string id = upper(target);
    if (auto match = registry_.find(id); match && match->state == State::trashed) {
        return *match;
    }
    std::vector<Workspace> candidates;
    for (auto& workspace : registry_.trashed()) {
        if (workspace.handle == target || workspace.id.starts_with(id)) {
            candidates.push_back(std::move(workspace));
        }
    }
    if (candidates.size() == 1) {
        return candidates.front();
    }
    if (candidates.size() > 1) {
        std::string list;
        for (const auto& workspace : candidates) {
            list += std::format("\n  {} {}", workspace.handle, workspace.id);
        }
        throw Error(
            ErrorKind::ambiguous,
            std::format("'{}' matches several trashed workspaces; use an ID:{}", target, list));
    }
    throw Error(ErrorKind::not_found, std::format("no trashed workspace matches '{}'", target));
}

std::vector<Workspace> Workspaces::restore(std::string_view target) {
    const auto lock = operation_lock();
    const Workspace top = resolve_trashed(target);
    check_restore_parent(registry_, top);

    // Everything removed in the same operation at or below `top`.
    std::set<std::string> below;
    for (const auto& workspace : registry_.subtree(top.id)) {
        below.insert(workspace.id);
    }
    std::vector<Workspace> batch;
    for (auto& workspace :
         top.removal_id ? registry_.removal(*top.removal_id) : std::vector<Workspace>{top}) {
        if (below.contains(workspace.id)) {
            batch.push_back(std::move(workspace));
        }
    }
    check_restore(registry_, batch);

    const std::vector<Workspace> original = batch;
    for (auto& workspace : batch) {
        workspace.state = State::active;
        workspace.removal_id.reset();
        workspace.updated_at = now_ms();
    }
    registry_.transaction([&] {
        for (const auto& workspace : batch) {
            registry_.update(workspace);
        }
    });
    std::vector<const Workspace*> moved;
    try {
        for (const auto& workspace : batch) {
            if (!workspace.trash_path) {
                throw Error(ErrorKind::inconsistent, "restore has no trash path");
            }
            move_path(*workspace.trash_path, workspace.path);
            moved.push_back(&workspace);
        }
    } catch (...) {
        rollback_restore(moved);
        registry_.transaction([&] {
            for (const auto& workspace : original) {
                registry_.update(workspace);
            }
        });
        throw;
    }
    for (auto& workspace : batch) {
        workspace.trash_path.reset();
    }
    registry_.transaction([&] {
        for (const auto& workspace : batch) {
            registry_.update(workspace);
        }
    });
    return batch;
}

GcResult Workspaces::gc() {
    const auto lock = operation_lock();
    GcResult result;
    for (auto& workspace : registry_.trashed()) {
        if (workspace.trash_path) {
            // Renaming first makes the deletion visible to `restore`, which
            // then refuses, and lets an interrupted gc resume.
            const fs::path deleting = deleting_location(*workspace.trash_path);
            if (!exists_nofollow(*workspace.trash_path) && exists_nofollow(workspace.path)) {
                throw Error(ErrorKind::inconsistent,
                            std::format("removing '{}' was interrupted; run `hz doctor --fix` "
                                        "before garbage collection",
                                        workspace.handle));
            }
            if (exists_nofollow(*workspace.trash_path)) {
                require_marker(*workspace.trash_path, workspace);
                move_path(*workspace.trash_path, deleting);
            }
            finish_gc(deleting, workspace);
            // Tidy up directories hz created, if now empty; never a
            // user-chosen --into directory.
            const fs::path trash_directory = workspace.trash_path->parent_path();
            try_rmdir(trash_directory);
            const fs::path storage = trash_directory.parent_path();
            if (storage.parent_path().filename() == storage_name) {
                try_rmdir(storage);
                try_rmdir(storage.parent_path());
            }
        }
        registry_.transaction([&] { registry_.erase(workspace.id); });
        result.deleted.push_back(std::move(workspace));
    }
    return result;
}

Workspace Workspaces::set_pinned(std::string_view target, bool pinned) {
    const auto lock = operation_lock();
    Workspace workspace = resolve(target);
    workspace.pinned = pinned;
    workspace.updated_at = now_ms();
    registry_.transaction([&] { registry_.update(workspace); });
    return workspace;
}

Workspace Workspaces::adopt(const fs::path& directory) {
    const auto lock = operation_lock();
    const fs::path path =
        canonical_directory(directory.is_absolute() ? directory : context_ / directory);
    const auto id = read_marker(path);
    if (!id) {
        throw Error(ErrorKind::not_found,
                    std::format("{} has no workspace marker to adopt", path.string()));
    }
    auto workspace = registry_.find(*id);
    if (!workspace) {
        throw Error(ErrorKind::not_found,
                    std::format("{} carries marker {}, which is not registered; use `hz init` "
                                "to register it as a new root",
                                path.string(), *id));
    }
    if (workspace->path == path) {
        return *workspace;
    }
    if (workspace->state != State::active) {
        throw Error(ErrorKind::conflict, std::format("workspace '{}' is {}", workspace->handle,
                                                     to_string(workspace->state)));
    }
    if (exists_nofollow(workspace->path) && carries_marker(workspace->path, workspace->id)) {
        throw Error(ErrorKind::conflict,
                    std::format("{} still carries the marker of '{}', so {} is a copy, not a "
                                "move; delete {}/{} from the copy",
                                workspace->path.string(), workspace->handle, path.string(),
                                path.string(), marker_name));
    }
    if (auto other = registry_.find_by_path(path)) {
        throw Error(ErrorKind::conflict, std::format("{} is registered as workspace '{}'",
                                                     path.string(), other->handle));
    }
    require_separate_directory(path, workspace->id);
    workspace->path = path;
    workspace->updated_at = now_ms();
    registry_.transaction([&] { registry_.update(*workspace); });
    return *workspace;
}

namespace {

// Recovery interprets the persisted intent one state at a time. It never
// guesses ownership from a registered path alone.
class Recovery {
  public:
    Recovery(Registry& registry, bool fix) : registry_(registry), fix_(fix) {}

    std::vector<Finding> run() {
        for (const auto& workspace : registry_.all()) {
            Finding finding{.workspace_id = workspace.id, .path = workspace.location()};
            switch (workspace.state) {
            case State::creating:
                creating(workspace, finding);
                break;
            case State::active:
                active(workspace, finding);
                break;
            case State::trashed:
                trashed(workspace, finding);
                break;
            }
        }
        orphans();
        return std::move(findings_);
    }

  private:
    void report(Finding finding) { findings_.push_back(std::move(finding)); }
    void erase(const Workspace& workspace) {
        registry_.transaction([&] { registry_.erase(workspace.id); });
    }
    void creating(Workspace workspace, Finding finding) {
        if (workspace.pid && process_alive(*workspace.pid)) {
            return; // still in progress
        }
        finding.kind = "interrupted_create";
        finding.message = std::format("creating '{}' was interrupted", workspace.handle);
        const auto marker = marker_or_nothing(workspace.path);
        if (exists_nofollow(workspace.path / marker_name) && marker != workspace.id) {
            finding.message += "; its directory carries another marker, so it was left";
        } else if (fix_) {
            remove_tree(workspace.path);
            erase(workspace);
            finding.fixed = true;
        }
        report(finding);
    }

    void active(Workspace workspace, Finding finding) {
        if (workspace.trash_path) {
            finding.kind = "interrupted_restore";
            finding.message = std::format("restoring '{}' was interrupted", workspace.handle);
            if (fix_) {
                if (!exists_nofollow(workspace.path) &&
                    carries_marker(*workspace.trash_path, workspace.id)) {
                    move_path(*workspace.trash_path, workspace.path);
                }
                if (carries_marker(workspace.path, workspace.id) &&
                    !exists_nofollow(*workspace.trash_path)) {
                    workspace.trash_path.reset();
                    registry_.transaction([&] { registry_.update(workspace); });
                    finding.fixed = true;
                }
            }
            report(finding);
        } else if (!exists_nofollow(workspace.path)) {
            finding.kind = "missing";
            finding.message = std::format("'{}' is missing from {}; if you moved it run `hz adopt "
                                          "NEW_PATH`, otherwise `hz rm {}` forgets it",
                                          workspace.handle, workspace.path.string(), workspace.id);
            report(finding);
        } else if (!carries_marker(workspace.path, workspace.id)) {
            finding.kind = "marker_mismatch";
            finding.message = std::format(
                "{} does not carry the marker of '{}'{}", workspace.path.string(), workspace.handle,
                workspace.is_root()
                    ? std::format("; if it is that root, `hz init {}` restores the marker",
                                  workspace.path.string())
                    : "");
            report(finding);
        }
        if (workspace.parent_id && !registry_.find(*workspace.parent_id)) {
            report(
                {.kind = "dangling_parent",
                 .message = std::format("the parent of '{}' is not registered", workspace.handle),
                 .workspace_id = workspace.id,
                 .path = workspace.path,
                 .fixed = false});
        }
    }

    void trashed(Workspace workspace, Finding finding) {
        if (!workspace.trash_path) {
            return; // its directory was already gone when it was removed
        }
        const fs::path deleting = deleting_location(*workspace.trash_path);
        if (exists_nofollow(deleting)) {
            finding.kind = "interrupted_gc";
            finding.message = std::format("deleting '{}' was interrupted", workspace.handle);
            if (exists_nofollow(*workspace.trash_path)) {
                finding.message += "; both trash and deletion paths exist, so both were left";
            } else if (fix_ && safe_to_finish_gc(deleting, workspace)) {
                finish_gc(deleting, workspace);
                erase(workspace);
                finding.fixed = true;
            }
            report(finding);
        } else if (!exists_nofollow(*workspace.trash_path)) {
            if (carries_marker(workspace.path, workspace.id)) {
                finding.kind = "interrupted_remove";
                finding.message = std::format("removing '{}' was interrupted", workspace.handle);
                if (fix_) {
                    make_private_directories(workspace.trash_path->parent_path());
                    move_path(workspace.path, *workspace.trash_path);
                    finding.fixed = true;
                }
            } else {
                finding.kind = "missing_trash";
                finding.message =
                    std::format("the trashed directory of '{}' is gone", workspace.handle);
                if (fix_) {
                    erase(workspace);
                    finding.fixed = true;
                }
            }
            report(finding);
        } else if (!carries_marker(*workspace.trash_path, workspace.id)) {
            finding.kind = "marker_mismatch";
            finding.message = std::format("{} does not carry the marker of '{}'",
                                          workspace.trash_path->string(), workspace.handle);
            report(finding);
        }
    }

    void scan_orphans(const fs::path& directory) {
        std::error_code error;
        const fs::directory_iterator entries(directory, error);
        if (error && error != std::errc::no_such_file_or_directory) {
            throw Error(ErrorKind::io,
                        std::format("inspect storage {}: {}", directory.string(), error.message()),
                        error);
        }
        for (const auto& entry : entries) {
            const std::string name = entry.path().filename().string();
            std::string id = name;
            if (id.ends_with(deleting_suffix)) {
                id.resize(id.size() - deleting_suffix.size());
            }
            if (!is_ulid(id) || registry_.find(id)) {
                continue;
            }
            const bool ours = marker_or_nothing(entry.path()) == id;
            Finding finding{.kind = "orphan",
                            .message = std::format("{} is not registered{}", entry.path().string(),
                                                   ours ? ""
                                                        : " and carries no matching "
                                                          "marker, so it was left"),
                            .workspace_id = id,
                            .path = entry.path(),
                            .fixed = false};
            if (fix_ && ours) {
                remove_tree(entry.path());
                finding.fixed = true;
            }
            report(finding);
        }
    }

    void orphans() {
        // Directories in storage that no row accounts for, typically left by an
        // interrupted gc. Only those whose own marker proves they were ours and
        // are no longer registered are deleted.
        std::set<fs::path> storages;
        for (const auto& workspace : registry_.all()) {
            if (workspace.is_root() && workspace.state == State::active) {
                storages.insert(Workspaces::storage_directory(workspace));
            }
        }
        for (const auto& storage : storages) {
            for (const auto& directory : {storage, storage / trash_name}) {
                scan_orphans(directory);
            }
        }
    }

    Registry& registry_;
    bool fix_;
    std::vector<Finding> findings_;
};

} // namespace

std::vector<Finding> Workspaces::doctor(bool fix) {
    const auto lock = operation_lock();
    return Recovery(registry_, fix).run();
}

} // namespace hz
