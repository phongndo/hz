#pragma once

#include <functional>
#include <string>
#include <string_view>

namespace hz {

// Handles are the names people type. They are 1 to 64 characters from
// [A-Za-z0-9._-], do not start with '.' or '-', and are not the reserved
// target keywords `root` and `local`, so every handle is also a safe
// directory name and an unquoted shell word.
bool is_valid_handle(std::string_view handle);
void require_valid_handle(std::string_view handle);

// A valid handle derived from a directory name, used for roots.
std::string handle_from_name(std::string_view name);

// A random adjective-noun handle for which `taken` returns false.
std::string generate_handle(const std::function<bool(std::string_view)>& taken);

} // namespace hz
