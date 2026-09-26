#pragma once

#include "hz/clone.hpp"
#include "hz/registry.hpp"

#include <filesystem>
#include <optional>
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

struct ListOptions {
    bool all_families = false; // otherwise the current family, or all outside one
    bool include_trashed = false;
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
    // From the root down to `workspace`'s parent.
    std::vector<Workspace> ancestors(const Workspace& workspace);

  private:
    Workspace workspace_at(const std::filesystem::path& directory);
    Workspace require_registered(const std::string& id);

    Registry registry_;
    std::filesystem::path context_;
};

// The canonical form of an existing directory; throws invalid_path otherwise.
std::filesystem::path canonical_directory(const std::filesystem::path& path);

} // namespace hz
