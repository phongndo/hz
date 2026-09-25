#pragma once

#include <chrono>
#include <string>
#include <string_view>

namespace hz {

// A ULID is 26 Crockford base32 characters: a 48-bit millisecond timestamp
// followed by 80 random bits. Workspace identities are ULIDs so that physical
// directory names sort by creation time and never collide.
std::string generate_ulid();
std::string generate_ulid(std::chrono::milliseconds timestamp);

// Accepts only the canonical uppercase encoding that hz writes.
bool is_ulid(std::string_view text);

} // namespace hz
