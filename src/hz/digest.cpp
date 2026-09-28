#include "hz/detail/digest.hpp"

#include <algorithm>
#include <bit>
#include <cstddef>
#include <vector>

#ifdef __APPLE__
#include <CommonCrypto/CommonDigest.h>
#endif

namespace hz::detail {

// The rounds index fixed-size message schedules and states by loop counters
// the loop bounds keep in range.
// NOLINTBEGIN(cppcoreguidelines-pro-bounds-constant-array-index)

#ifdef __APPLE__

// CommonCrypto uses the CPU's SHA instructions, several times faster than the
// portable code below. SHA-1 is deprecated there for security uses; Git's
// index checksum is not one.
#pragma clang diagnostic push
#pragma clang diagnostic ignored "-Wdeprecated-declarations"

std::array<std::uint8_t, 20> sha1(std::span<const std::uint8_t> data) {
    CC_SHA1_CTX context;
    CC_SHA1_Init(&context);
    for (std::size_t at = 0; at < data.size(); at += 1U << 30) {
        const auto length = std::min<std::size_t>(data.size() - at, 1U << 30);
        CC_SHA1_Update(&context, data.data() + at, static_cast<CC_LONG>(length));
    }
    std::array<std::uint8_t, 20> digest{};
    CC_SHA1_Final(digest.data(), &context);
    return digest;
}

#pragma clang diagnostic pop

std::array<std::uint8_t, 32> sha256(std::span<const std::uint8_t> data) {
    CC_SHA256_CTX context;
    CC_SHA256_Init(&context);
    for (std::size_t at = 0; at < data.size(); at += 1U << 30) {
        const auto length = std::min<std::size_t>(data.size() - at, 1U << 30);
        CC_SHA256_Update(&context, data.data() + at, static_cast<CC_LONG>(length));
    }
    std::array<std::uint8_t, 32> digest{};
    CC_SHA256_Final(digest.data(), &context);
    return digest;
}

#else

namespace {

// The message padded to whole 64-byte blocks, with its bit length appended
// big-endian, as both digests require.
std::vector<std::uint8_t> padded(std::span<const std::uint8_t> data) {
    std::vector<std::uint8_t> message(data.begin(), data.end());
    const std::uint64_t bits = static_cast<std::uint64_t>(data.size()) * 8;
    message.push_back(0x80);
    while (message.size() % 64 != 56) {
        message.push_back(0);
    }
    for (int shift = 56; shift >= 0; shift -= 8) {
        message.push_back(static_cast<std::uint8_t>(bits >> shift));
    }
    return message;
}

std::uint32_t load_big_endian(const std::uint8_t* bytes) {
    return (static_cast<std::uint32_t>(bytes[0]) << 24) |
           (static_cast<std::uint32_t>(bytes[1]) << 16) |
           (static_cast<std::uint32_t>(bytes[2]) << 8) | static_cast<std::uint32_t>(bytes[3]);
}

template <std::size_t N>
std::array<std::uint8_t, N * 4> store_big_endian(const std::array<std::uint32_t, N>& words) {
    std::array<std::uint8_t, N * 4> bytes{};
    for (std::size_t i = 0; i < N; ++i) {
        bytes[i * 4] = static_cast<std::uint8_t>(words[i] >> 24);
        bytes[(i * 4) + 1] = static_cast<std::uint8_t>(words[i] >> 16);
        bytes[(i * 4) + 2] = static_cast<std::uint8_t>(words[i] >> 8);
        bytes[(i * 4) + 3] = static_cast<std::uint8_t>(words[i]);
    }
    return bytes;
}

constexpr std::array<std::uint32_t, 64> sha256_constants{
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2};

} // namespace

std::array<std::uint8_t, 20> sha1(std::span<const std::uint8_t> data) {
    std::array<std::uint32_t, 5> state{0x67452301, 0xefcdab89, 0x98badcfe, 0x10325476, 0xc3d2e1f0};
    const auto message = padded(data);
    std::array<std::uint32_t, 80> w{};
    for (std::size_t block = 0; block < message.size(); block += 64) {
        for (std::size_t i = 0; i < 16; ++i) {
            w[i] = load_big_endian(&message[block + (i * 4)]);
        }
        for (std::size_t i = 16; i < 80; ++i) {
            w[i] = std::rotl(w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16], 1);
        }
        auto [a, b, c, d, e] = state;
        for (std::size_t i = 0; i < 80; ++i) {
            std::uint32_t f = 0;
            std::uint32_t k = 0;
            if (i < 20) {
                f = (b & c) | (~b & d);
                k = 0x5a827999;
            } else if (i < 40) {
                f = b ^ c ^ d;
                k = 0x6ed9eba1;
            } else if (i < 60) {
                f = (b & c) | (b & d) | (c & d);
                k = 0x8f1bbcdc;
            } else {
                f = b ^ c ^ d;
                k = 0xca62c1d6;
            }
            const std::uint32_t next = std::rotl(a, 5) + f + e + k + w[i];
            e = d;
            d = c;
            c = std::rotl(b, 30);
            b = a;
            a = next;
        }
        state[0] += a;
        state[1] += b;
        state[2] += c;
        state[3] += d;
        state[4] += e;
    }
    return store_big_endian(state);
}

std::array<std::uint8_t, 32> sha256(std::span<const std::uint8_t> data) {
    std::array<std::uint32_t, 8> state{0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a,
                                       0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19};
    const auto message = padded(data);
    std::array<std::uint32_t, 64> w{};
    for (std::size_t block = 0; block < message.size(); block += 64) {
        for (std::size_t i = 0; i < 16; ++i) {
            w[i] = load_big_endian(&message[block + (i * 4)]);
        }
        for (std::size_t i = 16; i < 64; ++i) {
            const std::uint32_t s0 =
                std::rotr(w[i - 15], 7) ^ std::rotr(w[i - 15], 18) ^ (w[i - 15] >> 3);
            const std::uint32_t s1 =
                std::rotr(w[i - 2], 17) ^ std::rotr(w[i - 2], 19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16] + s0 + w[i - 7] + s1;
        }
        auto [a, b, c, d, e, f, g, h] = state;
        for (std::size_t i = 0; i < 64; ++i) {
            const std::uint32_t s1 = std::rotr(e, 6) ^ std::rotr(e, 11) ^ std::rotr(e, 25);
            const std::uint32_t choose = (e & f) ^ (~e & g);
            const std::uint32_t t1 = h + s1 + choose + sha256_constants[i] + w[i];
            const std::uint32_t s0 = std::rotr(a, 2) ^ std::rotr(a, 13) ^ std::rotr(a, 22);
            const std::uint32_t majority = (a & b) ^ (a & c) ^ (b & c);
            const std::uint32_t t2 = s0 + majority;
            h = g;
            g = f;
            f = e;
            e = d + t1;
            d = c;
            c = b;
            b = a;
            a = t1 + t2;
        }
        state[0] += a;
        state[1] += b;
        state[2] += c;
        state[3] += d;
        state[4] += e;
        state[5] += f;
        state[6] += g;
        state[7] += h;
    }
    return store_big_endian(state);
}

#endif

// NOLINTEND(cppcoreguidelines-pro-bounds-constant-array-index)

} // namespace hz::detail
