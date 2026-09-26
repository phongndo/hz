#pragma once

#include <cstdint>
#include <filesystem>
#include <optional>
#include <string>
#include <string_view>
#include <type_traits>

struct sqlite3;
struct sqlite3_stmt;

namespace hz::sqlite {

class Statement;

// One connection to an SQLite database. Failures throw Error(registry).
class Database {
  public:
    explicit Database(const std::filesystem::path& path);
    ~Database();
    Database(const Database&) = delete;
    Database& operator=(const Database&) = delete;
    Database(Database&&) = delete;
    Database& operator=(Database&&) = delete;

    void exec(std::string_view sql);
    Statement prepare(std::string_view sql);
    [[nodiscard]] int changes() const;

    // Runs `body` inside BEGIN IMMEDIATE ... COMMIT, rolling back if it
    // throws. IMMEDIATE takes the write lock up front, so concurrent hz
    // processes queue on the busy timeout instead of failing to upgrade.
    template <class Body> auto transaction(Body&& body) {
        exec("BEGIN IMMEDIATE");
        try {
            if constexpr (std::is_void_v<std::invoke_result_t<Body>>) {
                std::forward<Body>(body)();
                exec("COMMIT");
            } else {
                auto result = std::forward<Body>(body)();
                exec("COMMIT");
                return result;
            }
        } catch (...) {
            rollback();
            throw;
        }
    }

  private:
    friend class Statement;
    void rollback() noexcept;
    [[noreturn]] void fail(std::string_view what) const;

    sqlite3* db_ = nullptr;
    std::filesystem::path path_;
};

// A prepared statement. Parameters are 1-based, columns are 0-based.
class Statement {
  public:
    ~Statement();
    Statement(const Statement&) = delete;
    Statement& operator=(const Statement&) = delete;
    Statement(Statement&& other) noexcept;
    Statement& operator=(Statement&&) = delete;

    Statement& bind(int index, std::string_view value);
    Statement& bind(int index, const std::string& value) {
        return bind(index, std::string_view(value));
    }
    Statement& bind(int index, std::int64_t value);
    Statement& bind(int index, std::nullopt_t null);
    Statement& bind(int index, const std::optional<std::string>& value);

    // Advances to the next row; false once the statement is done.
    bool step();
    // Runs a statement that returns no rows.
    void run();

    [[nodiscard]] bool is_null(int column) const;
    [[nodiscard]] std::string text(int column) const;
    [[nodiscard]] std::optional<std::string> optional_text(int column) const;
    [[nodiscard]] std::int64_t integer(int column) const;

  private:
    friend class Database;
    Statement(Database& db, sqlite3_stmt* statement) : db_(&db), statement_(statement) {}

    Database* db_;
    sqlite3_stmt* statement_;
};

} // namespace hz::sqlite
