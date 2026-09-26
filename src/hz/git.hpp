#pragma once

#include <filesystem>
#include <optional>
#include <string>
#include <vector>

// Git integration. Creation-time work (checks, detaching HEAD, marker
// exclusion) reads and writes .git files directly so the create path never
// spawns a process; explicit commands such as status and handoff run git.
namespace hz::git {

// The ref recording the commit a child workspace was created from.
inline constexpr std::string_view base_ref = "refs/hz/base";

// Whether `workspace` is the top of a Git checkout with a .git directory.
bool is_repository(const std::filesystem::path& workspace);

// Refuses sources whose copy would not be a coherent repository: linked
// worktrees and repositories with a Git operation in progress.
void check_source(const std::filesystem::path& workspace);

// Hides the workspace marker from Git and Mercurial in a root.
void prepare_root(const std::filesystem::path& workspace);

// In a freshly copied child: detaches HEAD at the commit it pointed to,
// records that commit as refs/hz/base, and hides the marker.
void prepare_child(const std::filesystem::path& workspace);

struct StatusEntry {
    std::string code; // porcelain v1 XY
    std::string path;
};

struct Status {
    std::optional<std::string> branch; // empty when detached or unborn
    std::optional<std::string> head;   // empty when unborn
    std::optional<std::string> base;   // refs/hz/base, if recorded
    std::vector<StatusEntry> entries;

    [[nodiscard]] bool dirty() const { return !entries.empty(); }
};

Status status(const std::filesystem::path& workspace);

struct Handoff {
    bool changed = false;
    std::string base; // the commit the patch was taken against
};

// Applies everything `from` changed since it was created (commits, staged,
// unstaged, and untracked files) to the clean checkout `to` as uncommitted
// changes. `three_way` lets git fall back to a three-way merge.
Handoff handoff(const std::filesystem::path& from, const std::filesystem::path& to, bool three_way);

// `hg status` output for a Mercurial workspace.
std::string hg_status(const std::filesystem::path& workspace);

} // namespace hz::git
