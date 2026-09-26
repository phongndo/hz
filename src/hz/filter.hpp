#pragma once

#include <filesystem>

namespace hz {

// Whether `relative`, a path below a workspace root, is a regenerable artifact
// that filtered copies omit. Matches directory names at any depth, so a source
// directory that happens to be called `build` or `dist` is skipped too.
// Nothing below source-control metadata is filtered except a live Git
// fsmonitor socket, which cannot be copied and would have no daemon behind it.
bool filter_excludes(const std::filesystem::path& relative);

} // namespace hz
