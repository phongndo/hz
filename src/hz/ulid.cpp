#include "hz/ulid.hpp"

#include <algorithm>
#include <cstdint>
#include <random>

namespace hz {

namespace {

constexpr std::string_view alphabet = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";
constexpr std::size_t ulid_length = 26;
constexpr std::size_t timestamp_length = 10;
constexpr std::uint64_t timestamp_mask = (std::uint64_t{1} << 48) - 1;

// Hands out five random bits at a time from 64-bit refills of the OS entropy
// source, so one ULID costs a handful of reads rather than one per character.
class RandomBits {
  public:
    unsigned next() {
        if (available_ < 5) {
            pool_ = (std::uint64_t{device_()} << 32) | device_();
            available_ = 64;
        }
        auto value = static_cast<unsigned>(pool_ & 31);
        pool_ >>= 5;
        available_ -= 5;
        return value;
    }

  private:
    std::random_device device_;
    std::uint64_t pool_ = 0;
    unsigned available_ = 0;
};

} // namespace

std::string generate_ulid() {
    auto now = std::chrono::system_clock::now().time_since_epoch();
    return generate_ulid(std::chrono::duration_cast<std::chrono::milliseconds>(now));
}

std::string generate_ulid(std::chrono::milliseconds timestamp) {
    std::string out(ulid_length, '0');
    auto millis = static_cast<std::uint64_t>(timestamp.count()) & timestamp_mask;
    for (std::size_t index = timestamp_length; index-- > 0;) {
        out[index] = alphabet[millis & 31];
        millis >>= 5;
    }
    thread_local RandomBits random;
    for (std::size_t index = timestamp_length; index < ulid_length; ++index) {
        out[index] = alphabet[random.next()];
    }
    return out;
}

bool is_ulid(std::string_view text) {
    if (text.size() != ulid_length || text.front() > '7') {
        return false;
    }
    return std::ranges::all_of(text,
                               [](char c) { return alphabet.find(c) != std::string_view::npos; });
}

} // namespace hz
