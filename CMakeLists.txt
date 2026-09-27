cmake_minimum_required(VERSION 3.28)

project(
  hz
  VERSION 0.10.0
  DESCRIPTION "Copy-on-write workspaces for parallel humans and agents"
  LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 23)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_CXX_EXTENSIONS OFF)
set(CMAKE_EXPORT_COMPILE_COMMANDS ON)
# No C++ modules yet; scanning adds GCC-only flags that clang-tidy rejects.
set(CMAKE_CXX_SCAN_FOR_MODULES OFF)

option(HZ_BUILD_TESTS "Build the hz test suites" ${PROJECT_IS_TOP_LEVEL})

find_package(CLI11 CONFIG REQUIRED)
find_package(tomlplusplus CONFIG REQUIRED)
find_package(nlohmann_json CONFIG REQUIRED)
find_package(SQLite3 REQUIRED)
find_package(Threads REQUIRED)

add_library(hz_warnings INTERFACE)
target_compile_options(
  hz_warnings
  INTERFACE
    $<$<CXX_COMPILER_ID:GNU,Clang,AppleClang>:-Wall;-Wextra;-Wpedantic;-Wshadow;-Wconversion;-Wsign-conversion>
    # Designated initializers that leave defaulted fields out are intended.
    $<$<CXX_COMPILER_ID:Clang,AppleClang>:-Wno-missing-designated-field-initializers>)

configure_file(src/hz/version.hpp.in include/hz/version.hpp @ONLY)

# hz_core: everything except argument parsing and output formatting.
add_library(
  hz_core STATIC
  src/hz/clone.cpp
  src/hz/config.cpp
  src/hz/error.cpp
  src/hz/filter.cpp
  src/hz/fsutil.cpp
  src/hz/git.cpp
  src/hz/handle.cpp
  src/hz/lifecycle.cpp
  src/hz/marker.cpp
  src/hz/metadata.cpp
  src/hz/process.cpp
  src/hz/registry.cpp
  src/hz/sqlite.cpp
  src/hz/tree.cpp
  src/hz/ulid.cpp
  src/hz/work_queue.cpp
  src/hz/workspaces.cpp)
target_include_directories(hz_core PUBLIC src ${CMAKE_CURRENT_BINARY_DIR}/include)
target_link_libraries(
  hz_core
  PUBLIC tomlplusplus::tomlplusplus nlohmann_json::nlohmann_json SQLite::SQLite3 Threads::Threads
  PRIVATE hz_warnings)

# Embed the shell integration scripts. Regenerated when a script changes;
# configure_file only touches the header when its content differs.
set(hz_shell_header "#pragma once\n\n#include <string_view>\n\nnamespace hz::cli {\n\n")
foreach(shell zsh bash fish)
  file(READ shell/hz.${shell} hz_script)
  string(APPEND hz_shell_header
         "inline constexpr std::string_view ${shell}_script = R\"hzshell(${hz_script})hzshell\";\n\n")
  set_property(DIRECTORY APPEND PROPERTY CMAKE_CONFIGURE_DEPENDS shell/hz.${shell})
endforeach()
string(APPEND hz_shell_header "} // namespace hz::cli\n")
file(WRITE ${CMAKE_CURRENT_BINARY_DIR}/shell_scripts.hpp.tmp "${hz_shell_header}")
configure_file(${CMAKE_CURRENT_BINARY_DIR}/shell_scripts.hpp.tmp
               ${CMAKE_CURRENT_BINARY_DIR}/include/cli/shell_scripts.hpp COPYONLY)

add_executable(
  hz
  src/cli/commands.cpp
  src/cli/integration_commands.cpp
  src/cli/main.cpp
  src/cli/output.cpp
  src/cli/workspace_commands.cpp)
target_include_directories(hz PRIVATE src)
target_link_libraries(hz PRIVATE hz_core CLI11::CLI11 hz_warnings)

install(TARGETS hz)

if(HZ_BUILD_TESTS)
  enable_testing()
  add_subdirectory(tests)
endif()
