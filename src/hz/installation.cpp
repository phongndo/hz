#include "hz/installation.hpp"

#include <nlohmann/json.hpp>

#include <vector>

namespace hz {

namespace fs = std::filesystem;

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

namespace {

bool is_below(const fs::path& path, const fs::path& directory) {
    const auto relative = path.lexically_relative(directory);
    return !relative.empty() && relative != "." && *relative.begin() != "..";
}

// mise keeps each tool in $MISE_DATA_DIR/installs, ~/.local/share/mise/installs by default.
bool is_in_mise(const fs::path& executable, std::string_view data_dir) {
    if (data_dir.empty()) {
        return executable.string().contains("/mise/installs/");
    }
    return is_below(executable, (fs::path(data_dir) / "installs").lexically_normal());
}

} // namespace

InstalledBy installed_by(const fs::path& executable, std::string_view mise_data_dir,
                         bool has_receipt) {
    if (has_receipt) {
        return {.installer = Installer::install_script};
    }
    const std::vector<fs::path> parts(executable.begin(), executable.end());
    // <prefix>/Cellar/<formula>/<version>/bin/hz; the prefix includes the root.
    for (size_t i = 1; i + 3 < parts.size(); ++i) {
        if (parts[i] == "Cellar") {
            fs::path prefix;
            for (size_t j = 0; j < i; ++j) {
                prefix /= parts[j];
            }
            return {.installer = Installer::homebrew,
                    .homebrew_prefix = prefix,
                    .homebrew_formula = parts[i + 1].string()};
        }
    }
    if (is_in_mise(executable, mise_data_dir)) {
        return {.installer = Installer::mise};
    }
    if (parts.size() > 4 && parts[0] == "/" && parts[1] == "nix" && parts[2] == "store") {
        return {.installer = Installer::nix, .nix_store_path = fs::path("/nix/store") / parts[3]};
    }
    return {};
}

std::optional<std::string> nix_profile_element(std::string_view profile_list,
                                               const fs::path& store_path) {
    // {"version": 3, "elements": {"<name>": {"storePaths": ["/nix/store/..."], ...}}}. Older
    // versions list the elements in an array, without the names `nix profile upgrade` needs.
    const auto document = nlohmann::json::parse(profile_list, nullptr, false);
    const auto elements = document.find("elements");
    if (elements == document.end() || !elements->is_object()) {
        return std::nullopt;
    }
    for (const auto& [name, element] : elements->items()) {
        const auto paths = element.find("storePaths");
        if (paths == element.end() || !paths->is_array()) {
            continue;
        }
        for (const auto& path : *paths) {
            if (path.is_string() && path.get<std::string>() == store_path.string()) {
                return name;
            }
        }
    }
    return std::nullopt;
}

} // namespace hz
