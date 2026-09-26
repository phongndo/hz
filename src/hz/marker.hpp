#pragma once

#include <filesystem>
#include <optional>
#include <string>
#include <string_view>

namespace hz {

// The identity marker every workspace directory carries.
inline constexpr std::string_view marker_name = ".hz-workspace";

// Writes `id` as the marker of `directory`, replacing any existing marker
// atomically.
void write_marker(const std::filesystem::path& directory, std::string_view id);

// The ID in `directory`'s marker, or nothing if it has none. Throws
// Error(inconsistent) if the marker exists but does not hold a ULID.
std::optional<std::string> read_marker(const std::filesystem::path& directory);

void remove_marker(const std::filesystem::path& directory);

// The nearest directory at or above `start` that has a marker.
std::optional<std::filesystem::path> find_marked_directory(const std::filesystem::path& start);

} // namespace hz
