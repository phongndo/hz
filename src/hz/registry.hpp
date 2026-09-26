#pragma once

#include "hz/clone.hpp"
#include "hz/sqlite.hpp"

#include <cstdint>
#include <filesystem>
#include <optional>
#include <string>
#include <string_view>
#include <vector>

namespace hz {

enum class State {
    creating, // registered, materialization in progress or interrupted
    active,   // usable
    trashed,  // moved into trash; restorable until garbage collected
};

std::string_view to_string(State state);
std::string_view to_string(CopyMode mode);

struct Workspace {
    std::string id;
    std::string root_id;
    std::optional<std::string> parent_id; // empty for a root
    std::string handle;
    std::filesystem::path path;                      // location while active
    std::optional<std::filesystem::path> trash_path; // location while trashed or being restored
    std::optional<std::string> removal_id;           // shared by workspaces trashed together
    State state = State::creating;
    CopyMode mode = CopyMode::cow;
    bool filtered = false;
    bool pinned = false;
    std::int64_t created_at = 0; // unix milliseconds
    std::int64_t updated_at = 0;
    std::optional<std::int64_t> pid; // creating process while creating

    [[nodiscard]] bool is_root() const { return !parent_id; }
    // Where the directory is when no operation is in flight.
    [[nodiscard]] const std::filesystem::path& location() const {
        return state == State::trashed && trash_path ? *trash_path : path;
    }
};

std::int64_t now_ms();

// The per-user SQLite registry of every workspace. Every method is a single
// statement; callers compose them inside `transaction`.
class Registry {
  public:
    explicit Registry(const std::filesystem::path& database);

    template <class Body> auto transaction(Body&& body) {
        return db_.transaction(std::forward<Body>(body));
    }

    void insert(const Workspace& workspace);
    void update(const Workspace& workspace);
    void erase(std::string_view id);

    [[nodiscard]] std::optional<Workspace> find(std::string_view id);
    [[nodiscard]] std::optional<Workspace> find_by_path(const std::filesystem::path& path);
    // Workspaces other than trashed ones holding `handle` within a family.
    [[nodiscard]] std::optional<Workspace> find_handle(std::string_view root_id,
                                                       std::string_view handle);
    [[nodiscard]] std::vector<Workspace> find_handle_anywhere(std::string_view handle);
    [[nodiscard]] std::vector<Workspace> find_id_prefix(std::string_view prefix);
    [[nodiscard]] std::vector<Workspace> find_trashed_handle(std::string_view root_id,
                                                             std::string_view handle);
    [[nodiscard]] std::vector<Workspace> removal(std::string_view removal_id);
    [[nodiscard]] std::vector<Workspace> trashed();

    [[nodiscard]] std::vector<Workspace> all();
    [[nodiscard]] std::vector<Workspace> family(std::string_view root_id);
    [[nodiscard]] std::vector<Workspace> children(std::string_view id);
    // `id` and all its descendants, deepest first.
    [[nodiscard]] std::vector<Workspace> subtree(std::string_view id);

  private:
    std::vector<Workspace> query(std::string_view where, std::string_view argument);

    sqlite::Database db_;
};

} // namespace hz
