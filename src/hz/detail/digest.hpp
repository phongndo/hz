#pragma once

#include <array>
#include <cstdint>
#include <span>

namespace hz::detail {

// One-shot digests for checksumming Git index files.
std::array<std::uint8_t, 20> sha1(std::span<const std::uint8_t> data);
std::array<std::uint8_t, 32> sha256(std::span<const std::uint8_t> data);

} // namespace hz::detail
