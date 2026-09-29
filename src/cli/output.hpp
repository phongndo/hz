#pragma once

#include "hz/error.hpp"
#include "hz/registry.hpp"

#include <nlohmann/json.hpp>

#include <optional>
#include <string>
#include <vector>

namespace hz::cli {

// Version of the JSON documents and exit statuses described in docs/cli.md.
// Bump it when a change could break a client: removing or renaming a field,
// changing a field's type or meaning, or changing an exit status.
inline constexpr int api_version = 1;

// Exit statuses besides success and the per-kind statuses of exit_status.
inline constexpr int exit_internal = 1;
inline constexpr int exit_usage = 2;
int exit_status(ErrorKind kind);

nlohmann::json to_json(const Workspace& workspace);
nlohmann::json to_json(const std::vector<Workspace>& workspaces);

// Everything a command prints goes through Output so that --json and
// --machine produce one JSON document on stdout and nothing else.
class Output {
  public:
    explicit Output(bool json) : json_(json) {}

    [[nodiscard]] bool json() const { return json_; }

    void emit(const nlohmann::json& document) const;
    void line(const std::string& text) const;
    void error(const Error& error) const;
    void error(const std::string& message) const;

    // One line per workspace; `current` is marked with '*'.
    void table(const std::vector<Workspace>& workspaces,
               const std::optional<std::string>& current) const;
    // Families drawn as trees, roots first.
    void tree(const std::vector<Workspace>& workspaces,
              const std::optional<std::string>& current) const;

  private:
    bool json_;
};

} // namespace hz::cli
