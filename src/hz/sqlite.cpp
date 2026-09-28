#include "hz/sqlite.hpp"

#include "hz/error.hpp"

#include <sqlite3.h>

#include <format>
#include <utility>

namespace hz::sqlite {

namespace {

constexpr int busy_timeout_ms = 10'000;

} // namespace

Database::Database(const std::filesystem::path& path) : path_(path) {
    const int flags = SQLITE_OPEN_READWRITE | SQLITE_OPEN_CREATE | SQLITE_OPEN_FULLMUTEX;
    if (sqlite3_open_v2(path.c_str(), &db_, flags, nullptr) != SQLITE_OK) {
        std::string message = db_ != nullptr ? sqlite3_errmsg(db_) : "out of memory";
        sqlite3_close(db_);
        db_ = nullptr;
        throw Error(ErrorKind::registry,
                    std::format("cannot open registry {}: {}", path.string(), message));
    }
    sqlite3_busy_timeout(db_, busy_timeout_ms);
    exec("PRAGMA journal_mode = WAL");
    exec("PRAGMA synchronous = NORMAL");
    // Every hz command opens and closes the registry. Checkpointing the log
    // on close would sync it each time; with synchronous = NORMAL recent
    // commits are not durable across power loss anyway, and SQLite still
    // checkpoints as the log grows.
    sqlite3_db_config(db_, SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE, 1, nullptr);
    int persist = 1;
    sqlite3_file_control(db_, "main", SQLITE_FCNTL_PERSIST_WAL, &persist);
}

Database::~Database() {
    sqlite3_close(db_);
}

void Database::exec(std::string_view sql) {
    std::string owned(sql);
    if (sqlite3_exec(db_, owned.c_str(), nullptr, nullptr, nullptr) != SQLITE_OK) {
        fail(owned);
    }
}

Statement Database::prepare(std::string_view sql) {
    sqlite3_stmt* statement = nullptr;
    if (sqlite3_prepare_v2(db_, sql.data(), static_cast<int>(sql.size()), &statement, nullptr) !=
        SQLITE_OK) {
        fail(sql);
    }
    return {*this, statement};
}

int Database::changes() const {
    return sqlite3_changes(db_);
}

void Database::rollback() noexcept {
    sqlite3_exec(db_, "ROLLBACK", nullptr, nullptr, nullptr);
}

void Database::fail(std::string_view what) const {
    throw Error(ErrorKind::registry, std::format("registry {}: {} (while running: {})",
                                                 path_.string(), sqlite3_errmsg(db_), what));
}

Statement::~Statement() {
    sqlite3_finalize(statement_);
}

Statement::Statement(Statement&& other) noexcept
    : db_(other.db_), statement_(std::exchange(other.statement_, nullptr)) {}

Statement& Statement::bind(int index, std::string_view value) {
    if (sqlite3_bind_text(statement_, index, value.data(), static_cast<int>(value.size()),
                          SQLITE_TRANSIENT) != SQLITE_OK) {
        db_->fail("bind");
    }
    return *this;
}

Statement& Statement::bind(int index, std::int64_t value) {
    if (sqlite3_bind_int64(statement_, index, value) != SQLITE_OK) {
        db_->fail("bind");
    }
    return *this;
}

Statement& Statement::bind(int index, std::nullopt_t /*null*/) {
    if (sqlite3_bind_null(statement_, index) != SQLITE_OK) {
        db_->fail("bind");
    }
    return *this;
}

Statement& Statement::bind(int index, const std::optional<std::string>& value) {
    return value ? bind(index, std::string_view(*value)) : bind(index, std::nullopt);
}

bool Statement::step() {
    switch (sqlite3_step(statement_)) {
    case SQLITE_ROW:
        return true;
    case SQLITE_DONE:
        return false;
    default:
        db_->fail(sqlite3_sql(statement_));
    }
}

void Statement::run() {
    while (step()) {
    }
}

bool Statement::is_null(int column) const {
    return sqlite3_column_type(statement_, column) == SQLITE_NULL;
}

std::string Statement::text(int column) const {
    const auto* data = sqlite3_column_text(statement_, column);
    if (data == nullptr) {
        return {};
    }
    return {
        reinterpret_cast<const char*>(data), // NOLINT(cppcoreguidelines-pro-type-reinterpret-cast)
        static_cast<size_t>(sqlite3_column_bytes(statement_, column))};
}

std::optional<std::string> Statement::optional_text(int column) const {
    if (is_null(column)) {
        return std::nullopt;
    }
    return text(column);
}

std::int64_t Statement::integer(int column) const {
    return sqlite3_column_int64(statement_, column);
}

} // namespace hz::sqlite
