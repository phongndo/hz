#pragma once

#include "hz/error.hpp"
#include "hz/registry.hpp"

#include <nlohmann/json.hpp>

#include <optional>
#include <string>
#include <vector>

namespace hz::cli {

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
