#pragma once

#include <map>
#include <optional>
#include <string>
#include <string_view>
#include <vector>

namespace hz {

// Key-value pairs that callers attach to a workspace, such as the ID of the
// agent session or task that owns it. hz stores and filters them but gives
// them no meaning, and children do not inherit them.
//
// Keys are 1 to 128 characters from [A-Za-z0-9._/-] starting with a letter or
// digit. Values are at most 4096 bytes of UTF-8 without C0 or C1 control
// characters, so they print safely on a terminal.
using Labels = std::map<std::string, std::string, std::less<>>;

// Selects workspaces whose label `key` exists and, when `value` is set,
// equals it.
struct LabelSelector {
    std::string key;
    std::optional<std::string> value;
};

void require_valid_label_key(std::string_view key);
void require_valid_label(std::string_view key, std::string_view value);
void require_valid_labels(const Labels& labels);

// Parses `KEY=VALUE`, splitting at the first '='.
std::pair<std::string, std::string> parse_label(std::string_view text);
// Parses `KEY=VALUE`, or `KEY` to require only that the key exists.
LabelSelector parse_label_selector(std::string_view text);

// Whether `labels` satisfies every selector.
bool matches(const Labels& labels, const std::vector<LabelSelector>& selectors);

} // namespace hz
