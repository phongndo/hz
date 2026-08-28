#include "app/application.hpp"

#include "hz/version.hpp"

#include <gtest/gtest.h>

#include <array>
#include <sstream>
#include <string>
#include <string_view>

namespace hz::app {
namespace {

TEST(ApplicationTest, ShowsHelpWithoutArguments) {
  std::ostringstream output;
  std::ostringstream error;

  const auto result = run({}, output, error);

  EXPECT_EQ(result, 0);
  EXPECT_NE(output.str().find("Usage: hz"), std::string::npos);
  EXPECT_TRUE(error.str().empty());
}

TEST(ApplicationTest, ShowsVersion) {
  constexpr std::array arguments{std::string_view{"--version"}};
  std::ostringstream output;
  std::ostringstream error;

  const auto result = run(arguments, output, error);

  EXPECT_EQ(result, 0);
  EXPECT_EQ(output.str(), "hz " + std::string{hz::version} + "\n");
  EXPECT_TRUE(error.str().empty());
}

TEST(ApplicationTest, RejectsUnsupportedArguments) {
  constexpr std::array arguments{std::string_view{"workspace"}};
  std::ostringstream output;
  std::ostringstream error;

  const auto result = run(arguments, output, error);

  EXPECT_EQ(result, 2);
  EXPECT_TRUE(output.str().empty());
  EXPECT_NE(error.str().find("unsupported argument: workspace"), std::string::npos);
}

} // namespace
} // namespace hz::app
