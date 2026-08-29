#ifndef LIGHT_APP_APPLICATION_HPP
#define LIGHT_APP_APPLICATION_HPP

#include <iosfwd>
#include <span>
#include <string_view>

namespace light::app {

[[nodiscard]] auto run(std::span<const std::string_view> arguments, std::ostream& output,
                       std::ostream& error) -> int;

[[nodiscard]] auto run(int argument_count, char** arguments) -> int;

} // namespace light::app

#endif // LIGHT_APP_APPLICATION_HPP
