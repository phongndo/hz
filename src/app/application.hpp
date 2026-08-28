#ifndef HZ_APP_APPLICATION_HPP
#define HZ_APP_APPLICATION_HPP

#include <iosfwd>
#include <span>
#include <string_view>

namespace hz::app {

[[nodiscard]] auto run(std::span<const std::string_view> arguments, std::ostream& output,
                       std::ostream& error) -> int;

[[nodiscard]] auto run(int argument_count, char** arguments) -> int;

} // namespace hz::app

#endif // HZ_APP_APPLICATION_HPP
