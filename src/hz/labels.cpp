#include "hz/labels.hpp"

#include "hz/error.hpp"

#include <nlohmann/json.hpp>

#include <algorithm>
#include <format>

namespace hz {

namespace {

constexpr size_t max_key_length = 128;
constexpr size_t max_value_length = 4096;

bool is_alphanumeric(char c) {
    return (c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z') || (c >= '0' && c <= '9');
}

bool is_key_character(char c) {
    return is_alphanumeric(c) || c == '.' || c == '_' || c == '/' || c == '-';
}

bool is_control(char c) {
    const auto byte = static_cast<unsigned char>(c);
    return byte < 0x20 || byte == 0x7f;
}

// C1 controls, U+0080 to U+009F, which some terminals also interpret. In valid
// UTF-8 they are exactly the two-byte sequences C2 80 to C2 9F.
bool has_c1_control(std::string_view text) {
    for (size_t i = 0; i + 1 < text.size(); ++i) {
        if (static_cast<unsigned char>(text[i]) == 0xc2 &&
            static_cast<unsigned char>(text[i + 1]) <= 0x9f) {
            return true;
        }
    }
    return false;
}

bool is_utf8(std::string_view text) {
    try {
        (void)nlohmann::json(text).dump();
        return true;
    } catch (const nlohmann::json::type_error&) {
        return false;
    }
}

} // namespace

void require_valid_label_key(std::string_view key) {
    if (key.empty() || key.size() > max_key_length || !is_alphanumeric(key.front()) ||
        !std::ranges::all_of(key, is_key_character)) {
        throw Error(ErrorKind::invalid_argument,
                    std::format("'{}' is not a valid label key: use 1-128 letters, digits, '.', "
                                "'_', '/', or '-', starting with a letter or digit",
                                key));
    }
}

void require_valid_label(std::string_view key, std::string_view value) {
    require_valid_label_key(key);
    if (value.size() > max_value_length || std::ranges::any_of(value, is_control) ||
        !is_utf8(value) || has_c1_control(value)) {
        throw Error(ErrorKind::invalid_argument,
                    std::format("the value of label '{}' must be at most {} bytes of UTF-8 "
                                "without control characters",
                                key, max_value_length));
    }
}

void require_valid_labels(const Labels& labels) {
    for (const auto& [key, value] : labels) {
        require_valid_label(key, value);
    }
}

std::pair<std::string, std::string> parse_label(std::string_view text) {
    const auto equals = text.find('=');
    if (equals == std::string_view::npos) {
        throw Error(ErrorKind::invalid_argument,
                    std::format("'{}' is not a label: use KEY=VALUE", text));
    }
    std::pair<std::string, std::string> label{text.substr(0, equals), text.substr(equals + 1)};
    require_valid_label(label.first, label.second);
    return label;
}

LabelSelector parse_label_selector(std::string_view text) {
    if (text.find('=') == std::string_view::npos) {
        require_valid_label_key(text);
        return {.key = std::string(text), .value = std::nullopt};
    }
    auto [key, value] = parse_label(text);
    return {.key = std::move(key), .value = std::move(value)};
}

bool matches(const Labels& labels, const std::vector<LabelSelector>& selectors) {
    return std::ranges::all_of(selectors, [&](const LabelSelector& selector) {
        const auto found = labels.find(selector.key);
        return found != labels.end() && (!selector.value || found->second == *selector.value);
    });
}

} // namespace hz
