#include "app/application.hpp"

#include <benchmark/benchmark.h>

#include <array>
#include <ostream>
#include <streambuf>
#include <string_view>

namespace {

class NullBuffer final : public std::streambuf {
protected:
  auto overflow(const int_type character) -> int_type override {
    return traits_type::not_eof(character);
  }
};

void cli_version(benchmark::State& state) {
  constexpr std::array arguments{std::string_view{"--version"}};
  NullBuffer buffer;
  std::ostream stream{&buffer};

  for ([[maybe_unused]] const auto iteration : state) {
    auto result = hz::app::run(arguments, stream, stream);
    benchmark::DoNotOptimize(result);
  }
}

BENCHMARK(cli_version);

} // namespace

BENCHMARK_MAIN();
