#include "hz/installation.hpp"

namespace hz {

std::optional<Installation> parse_installation(std::string_view receipt) {
    Installation found;
    while (!receipt.empty()) {
        const auto end = receipt.find('\n');
        const auto line = receipt.substr(0, end);
        receipt = end == std::string_view::npos ? std::string_view{} : receipt.substr(end + 1);
        const auto equals = line.find('=');
        if (equals == std::string_view::npos) {
            continue;
        }
        const auto key = line.substr(0, equals);
        const auto value = line.substr(equals + 1);
        if (key == "install_dir") {
            found.install_dir = value;
        } else if (key == "releases_dir") {
            found.releases_dir = value;
        }
    }
    if (!found.install_dir.is_absolute() || !found.releases_dir.is_absolute()) {
        return std::nullopt;
    }
    return found;
}

} // namespace hz
