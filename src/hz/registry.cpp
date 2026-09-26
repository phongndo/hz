#include "hz/registry.hpp"

#include "hz/error.hpp"

#include <chrono>
#include <format>

namespace hz {

namespace fs = std::filesystem;

namespace {

constexpr int schema_version = 2;

// Paths are absolute and canonical. Uniqueness of handles is per family and
// ignores trashed rows so a handle can be reused once its owner is removed.
// Parent and root references are not foreign keys: unregistering a root
// deletes its row while trashed descendants remain until garbage collection,
// and `hz doctor` checks the references instead.
constexpr std::string_view schema = R"sql(
CREATE TABLE workspace (
    id          TEXT PRIMARY KEY,
    root_id     TEXT NOT NULL,
    parent_id   TEXT,
    handle      TEXT NOT NULL,
    path        TEXT NOT NULL UNIQUE,
    trash_path  TEXT UNIQUE,
    removal_id  TEXT,
    state       TEXT NOT NULL CHECK (state IN ('creating', 'active', 'trashed')),
    mode        TEXT NOT NULL CHECK (mode IN ('cow', 'copy')),
    filtered    INTEGER NOT NULL DEFAULT 0,
    pinned      INTEGER NOT NULL DEFAULT 0,
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL,
    pid         INTEGER
) STRICT;
CREATE UNIQUE INDEX workspace_handle ON workspace (root_id, handle) WHERE state != 'trashed';
CREATE INDEX workspace_parent ON workspace (parent_id);
CREATE INDEX workspace_removal ON workspace (removal_id);
)sql";

constexpr std::string_view columns =
    "id, root_id, parent_id, handle, path, trash_path, state, "
    "mode, filtered, pinned, created_at, updated_at, pid, removal_id";

State parse_state(const std::string& text) {
    if (text == "active") {
        return State::active;
    }
    if (text == "trashed") {
        return State::trashed;
    }
    return State::creating;
}

Workspace read_row(const sqlite::Statement& row) {
    Workspace workspace;
    workspace.id = row.text(0);
    workspace.root_id = row.text(1);
    workspace.parent_id = row.optional_text(2);
    workspace.handle = row.text(3);
    workspace.path = row.text(4);
    if (auto trash = row.optional_text(5)) {
        workspace.trash_path = fs::path(*trash);
    }
    workspace.state = parse_state(row.text(6));
    workspace.mode = row.text(7) == "copy" ? CopyMode::copy : CopyMode::cow;
    workspace.filtered = row.integer(8) != 0;
    workspace.pinned = row.integer(9) != 0;
    workspace.created_at = row.integer(10);
    workspace.updated_at = row.integer(11);
    if (!row.is_null(12)) {
        workspace.pid = row.integer(12);
    }
    workspace.removal_id = row.optional_text(13);
    return workspace;
}

std::optional<std::string> optional_path(const std::optional<fs::path>& path) {
    if (!path) {
        return std::nullopt;
    }
    return path->string();
}

void bind_fields(sqlite::Statement& statement, const Workspace& workspace) {
    statement.bind(1, workspace.id)
        .bind(2, workspace.root_id)
        .bind(3, workspace.parent_id)
        .bind(4, workspace.handle)
        .bind(5, workspace.path.string())
        .bind(6, optional_path(workspace.trash_path))
        .bind(7, to_string(workspace.state))
        .bind(8, to_string(workspace.mode))
        .bind(9, std::int64_t{workspace.filtered ? 1 : 0})
        .bind(10, std::int64_t{workspace.pinned ? 1 : 0})
        .bind(11, workspace.created_at)
        .bind(12, workspace.updated_at);
    if (workspace.pid) {
        statement.bind(13, *workspace.pid);
    } else {
        statement.bind(13, std::nullopt);
    }
    statement.bind(14, workspace.removal_id);
}

} // namespace

std::string_view to_string(State state) {
    switch (state) {
    case State::creating:
        return "creating";
    case State::active:
        return "active";
    case State::trashed:
        return "trashed";
    }
    return "unknown";
}

std::string_view to_string(CopyMode mode) {
    return mode == CopyMode::copy ? "copy" : "cow";
}

std::int64_t now_ms() {
    return std::chrono::duration_cast<std::chrono::milliseconds>(
               std::chrono::system_clock::now().time_since_epoch())
        .count();
}

Registry::Registry(const fs::path& database) : db_(database) {
    db_.transaction([&] {
        auto version = db_.prepare("PRAGMA user_version");
        version.step();
        const auto current = version.integer(0);
        if (current == 0) {
            db_.exec(schema);
            db_.exec(std::format("PRAGMA user_version = {}", schema_version));
        } else if (current != schema_version) {
            throw Error(ErrorKind::registry,
                        std::format("registry {} has schema version {}, but this hz requires {}; "
                                    "use the matching hz version, or set HZ_DATA_DIR to a fresh "
                                    "directory and re-register roots with `hz init`",
                                    database.string(), current, schema_version));
        }
    });
}

void Registry::insert(const Workspace& workspace) {
    auto statement = db_.prepare(std::format("INSERT INTO workspace ({}) VALUES (?1, ?2, ?3, ?4, "
                                             "?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
                                             columns));
    bind_fields(statement, workspace);
    try {
        statement.run();
    } catch (const Error&) {
        if (find_handle(workspace.root_id, workspace.handle)) {
            throw Error(ErrorKind::conflict,
                        std::format("a workspace named '{}' already exists", workspace.handle));
        }
        if (find_by_path(workspace.path)) {
            throw Error(ErrorKind::conflict, std::format("{} is already registered as a workspace",
                                                         workspace.path.string()));
        }
        throw;
    }
}

void Registry::update(const Workspace& workspace) {
    auto statement = db_.prepare(
        "UPDATE workspace SET root_id = ?2, parent_id = ?3, handle = ?4, path = ?5, "
        "trash_path = ?6, state = ?7, mode = ?8, filtered = ?9, pinned = ?10, "
        "created_at = ?11, updated_at = ?12, pid = ?13, removal_id = ?14 WHERE id = ?1");
    bind_fields(statement, workspace);
    statement.run();
    if (db_.changes() != 1) {
        throw Error(ErrorKind::not_found,
                    std::format("workspace {} is no longer registered", workspace.id));
    }
}

void Registry::erase(std::string_view id) {
    db_.prepare("DELETE FROM workspace WHERE id = ?1").bind(1, id).run();
}

std::vector<Workspace> Registry::query(std::string_view where, std::string_view argument) {
    auto statement = db_.prepare(
        std::format("SELECT {} FROM workspace WHERE {} ORDER BY created_at, id", columns, where));
    statement.bind(1, argument);
    std::vector<Workspace> rows;
    while (statement.step()) {
        rows.push_back(read_row(statement));
    }
    return rows;
}

std::optional<Workspace> Registry::find(std::string_view id) {
    auto rows = query("id = ?1", id);
    if (rows.empty()) {
        return std::nullopt;
    }
    return rows.front();
}

std::optional<Workspace> Registry::find_by_path(const fs::path& path) {
    auto rows = query("path = ?1", path.string());
    if (rows.empty()) {
        return std::nullopt;
    }
    return rows.front();
}

std::optional<Workspace> Registry::find_handle(std::string_view root_id, std::string_view handle) {
    auto statement = db_.prepare(
        std::format("SELECT {} FROM workspace WHERE root_id = ?1 AND handle = ?2 AND state != "
                    "'trashed'",
                    columns));
    statement.bind(1, root_id).bind(2, handle);
    if (statement.step()) {
        return read_row(statement);
    }
    return std::nullopt;
}

std::vector<Workspace> Registry::find_handle_anywhere(std::string_view handle) {
    return query("handle = ?1 AND state != 'trashed'", handle);
}

std::vector<Workspace> Registry::find_id_prefix(std::string_view prefix) {
    // The caller supplies arbitrary target text, not a SQL LIKE pattern.
    return query("substr(id, 1, length(?1)) = ?1", prefix);
}

std::vector<Workspace> Registry::find_trashed_handle(std::string_view root_id,
                                                     std::string_view handle) {
    auto statement = db_.prepare(std::format("SELECT {} FROM workspace WHERE root_id = ?1 AND "
                                             "handle = ?2 AND state = 'trashed' ORDER BY "
                                             "updated_at DESC",
                                             columns));
    statement.bind(1, root_id).bind(2, handle);
    std::vector<Workspace> rows;
    while (statement.step()) {
        rows.push_back(read_row(statement));
    }
    return rows;
}

std::vector<Workspace> Registry::removal(std::string_view removal_id) {
    return query("removal_id = ?1", removal_id);
}

std::vector<Workspace> Registry::trashed() {
    return query("state = ?1", "trashed");
}

std::vector<Workspace> Registry::all() {
    return query("?1 = ?1", "");
}

std::vector<Workspace> Registry::family(std::string_view root_id) {
    return query("root_id = ?1", root_id);
}

std::vector<Workspace> Registry::children(std::string_view id) {
    return query("parent_id = ?1", id);
}

std::vector<Workspace> Registry::subtree(std::string_view id) {
    auto statement = db_.prepare(std::format(
        "WITH RECURSIVE tree(id, depth) AS ("
        "  SELECT id, 0 FROM workspace WHERE id = ?1"
        "  UNION ALL"
        "  SELECT w.id, tree.depth + 1 FROM workspace w JOIN tree ON w.parent_id = tree.id"
        ") SELECT {} FROM workspace JOIN tree USING (id) ORDER BY tree.depth DESC, created_at",
        columns));
    statement.bind(1, id);
    std::vector<Workspace> rows;
    while (statement.step()) {
        rows.push_back(read_row(statement));
    }
    return rows;
}

} // namespace hz
