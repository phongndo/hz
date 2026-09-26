#include "cli/output.hpp"

#include <algorithm>
#include <format>
#include <iostream>
#include <map>
#include <print>

namespace hz::cli {

namespace {

std::string notes(const Workspace& workspace) {
    std::vector<std::string> parts;
    if (workspace.is_root()) {
        parts.emplace_back("root");
    }
    if (workspace.state != State::active) {
        parts.emplace_back(to_string(workspace.state));
    }
    if (workspace.pinned) {
        parts.emplace_back("pinned");
    }
    if (workspace.filtered) {
        parts.emplace_back("filtered");
    }
    if (workspace.mode == CopyMode::copy) {
        parts.emplace_back("copy");
    }
    if (parts.empty()) {
        return {};
    }
    std::string text = "  [";
    for (size_t i = 0; i < parts.size(); ++i) {
        text += (i == 0 ? "" : ", ") + parts[i];
    }
    return text + "]";
}

} // namespace

nlohmann::json to_json(const Workspace& workspace) {
    nlohmann::json object{
        {"id", workspace.id},
        {"handle", workspace.handle},
        {"root_id", workspace.root_id},
        {"parent_id", nullptr},
        {"path", workspace.path.string()},
        {"location", workspace.location().string()},
        {"state", to_string(workspace.state)},
        {"mode", to_string(workspace.mode)},
        {"filtered", workspace.filtered},
        {"pinned", workspace.pinned},
        {"root", workspace.is_root()},
        {"created_at", workspace.created_at},
        {"updated_at", workspace.updated_at},
    };
    if (workspace.parent_id) {
        object["parent_id"] = *workspace.parent_id;
    }
    return object;
}

nlohmann::json to_json(const std::vector<Workspace>& workspaces) {
    auto array = nlohmann::json::array();
    for (const auto& workspace : workspaces) {
        array.push_back(to_json(workspace));
    }
    return array;
}

void Output::emit(const nlohmann::json& document) const {
    std::println("{}", document.dump(2));
}

void Output::line(const std::string& text) const {
    std::println("{}", text);
}

void Output::error(const Error& error) const {
    if (json_) {
        emit({{"error", {{"kind", to_string(error.kind())}, {"message", error.what()}}}});
    } else {
        std::cerr << "hz: " << error.what() << '\n';
    }
}

void Output::error(const std::string& message) const {
    if (json_) {
        emit({{"error", {{"kind", "internal"}, {"message", message}}}});
    } else {
        std::cerr << "hz: " << message << '\n';
    }
}

void Output::table(const std::vector<Workspace>& workspaces,
                   const std::optional<std::string>& current) const {
    size_t width = 0;
    for (const auto& workspace : workspaces) {
        width = std::max(width, workspace.handle.size());
    }
    for (const auto& workspace : workspaces) {
        const bool here = current && *current == workspace.id;
        std::println("{} {:<{}}  {}{}", here ? '*' : ' ', workspace.handle, width,
                     workspace.location().string(), notes(workspace));
    }
}

void Output::tree(const std::vector<Workspace>& workspaces,
                  const std::optional<std::string>& current) const {
    std::map<std::string, std::vector<const Workspace*>> children;
    std::vector<const Workspace*> tops;
    for (const auto& workspace : workspaces) {
        const bool parent_listed =
            workspace.parent_id && std::ranges::any_of(workspaces, [&](const Workspace& other) {
                return other.id == *workspace.parent_id;
            });
        if (parent_listed) {
            children[*workspace.parent_id].push_back(&workspace);
        } else {
            tops.push_back(&workspace);
        }
    }
    auto label = [&](const Workspace& workspace) {
        const bool here = current && *current == workspace.id;
        return std::format("{}{}  {}{}", workspace.handle, here ? " *" : "",
                           workspace.location().string(), notes(workspace));
    };
    auto draw = [&](auto& self, const Workspace& node, const std::string& prefix) -> void {
        const auto& kids = children[node.id];
        for (size_t i = 0; i < kids.size(); ++i) {
            const bool last = i + 1 == kids.size();
            std::println("{}{}{}", prefix, last ? "└── " : "├── ", label(*kids[i]));
            self(self, *kids[i], prefix + (last ? "    " : "│   "));
        }
    };
    for (size_t i = 0; i < tops.size(); ++i) {
        if (i > 0) {
            std::println("");
        }
        std::println("{}", label(*tops[i]));
        draw(draw, *tops[i], "");
    }
}

} // namespace hz::cli
