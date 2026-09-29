#pragma once

#include "hz/clone.hpp"
#include "hz/detail/fd.hpp"
#include "hz/error.hpp"
#include "hz/registry.hpp"

#include <filesystem>
#include <optional>
#include <set>
#include <string>
#include <string_view>
#include <vector>

namespace hz {

// Where the registry lives: $HZ_DATA_DIR, else the platform data directory
// ($XDG_DATA_HOME/hz or ~/.local/share/hz; ~/Library/Application Support/hz).
std::filesystem::path default_data_directory();

struct InitResult {
    Workspace workspace;
    bool created = false; // false when the directory was already a root
};

struct CreateOptions {
    std::string source;                        // target to copy; empty for current
    std::optional<std::string> handle;         // generated when empty
    std::optional<std::filesystem::path> into; // storage directory override
    std::optional<bool> filtered;              // default: the source's [create] filtered
    bool hooks = true;                         // run postcreate hooks
    Labels labels;                             // attached to the new workspace
};

struct RemoveOptions {
    std::string target;         // empty for current
    bool children_only = false; // keep the target, trash its descendants
    bool force = false;         // required to unregister a root
    bool hooks = true;          // run preremove hooks
};

struct RemoveResult {
    std::vector<Workspace> trashed;        // as they are now, in trash
    std::optional<Workspace> unregistered; // the root, if one was unregistered
    // Where a shell whose working directory was inside a removed workspace
    // should go: the nearest surviving ancestor.
    std::optional<std::filesystem::path> navigate_to;
};

struct GcResult {
    std::vector<Workspace> deleted;
};

// One problem found by `doctor`, and whether --fix repaired it.
struct Finding {
    std::string kind;
    std::string message;
    std::optional<std::string> workspace_id;
    std::optional<std::filesystem::path> path;
    bool fixed = false;
};

struct ListOptions {
    bool all_families = false; // otherwise the current family, or all outside one
    bool include_trashed = false;
    std::vector<LabelSelector> labels; // only workspaces matching all of them
};

// The workspace model: registration, lookup, and lifecycle. `context` is the
// directory that "current workspace" and relative targets are resolved from.
class Workspaces {
  public:
    Workspaces(const std::filesystem::path& data_directory, std::filesystem::path context);

    Registry& registry() { return registry_; }
    [[nodiscard]] const std::filesystem::path& context() const { return context_; }

    // Registers `directory` as a new root. `mode` cow verifies cloning works
    // there first; copy opts the family into byte copies.
    InitResult init(const std::filesystem::path& directory, CopyMode mode);

    // The workspace containing the context directory, if any.
    std::optional<Workspace> current_optional();
    Workspace current();

    // Resolves a target: empty for the current workspace, `root` or `local`
    // for its family root, a path, a handle, a full ID, or an unambiguous ID
    // prefix. Trashed workspaces are never matched.
    Workspace resolve(std::string_view target);

    std::vector<Workspace> list(const ListOptions& options);

    // Copies a workspace into a new child and registers it.
    Workspace create(const CreateOptions& options);

    // Moves a workspace and its descendants into trash, or unregisters a root.
    RemoveResult remove(const RemoveOptions& options);

    // Moves a trashed workspace, and everything removed with it, back.
    std::vector<Workspace> restore(std::string_view target);

    // Physically deletes everything in trash.
    GcResult gc();

    Workspace set_pinned(std::string_view target, bool pinned);

    // Sets the labels in `set` and removes the keys in `unset`, which must
    // not overlap. Unsetting a missing key does nothing.
    Workspace set_labels(std::string_view target, const Labels& set,
                         const std::vector<std::string>& unset);

    // Records that a workspace directory was moved to `directory`.
    Workspace adopt(const std::filesystem::path& directory);

    // Refuses unless `workspace` is active and no child is being copied from
    // it, so its contents may change. Call with the operation lock held.
    void require_quiescent(const Workspace& workspace);

    // Checks the registry against the filesystem; repairs what is provable.
    std::vector<Finding> doctor(bool fix);

    // The directory new children of `root` are stored in.
    static std::filesystem::path storage_directory(const Workspace& root);
    // From the root down to `workspace`'s parent.
    std::vector<Workspace> ancestors(const Workspace& workspace);

  private:
    detail::Fd operation_lock();
    void require_separate_directory(const std::filesystem::path& directory,
                                    std::string_view except_id = "");
    Workspace workspace_at(const std::filesystem::path& directory);
    Workspace require_registered(const std::string& id);
    Workspace resolve_trashed(std::string_view target);
    void trash(std::vector<Workspace>& batch);
    // Marks a copied child active, or says why its copy must be discarded.
    // Call with the operation lock held.
    std::optional<Error> activate(Workspace& child, const Workspace& source);
    // A trashed workspace this gc process now deletes, already moved to its
    // deletion path.
    struct GcClaim {
        Workspace workspace;
        detail::Fd lease;
        std::filesystem::path deleting;
    };
    std::optional<GcClaim> claim_for_gc(const std::string& id);
    void tidy_storage(const std::set<std::filesystem::path>& trash_directories);

    Registry registry_;
    std::filesystem::path context_;
    std::filesystem::path data_directory_;
};

// The canonical form of an existing directory; throws invalid_path otherwise.
std::filesystem::path canonical_directory(const std::filesystem::path& path);

} // namespace hz
