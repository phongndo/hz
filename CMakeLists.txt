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

add_library(hz_warnings INTERFACE)
target_compile_options(
  hz_warnings
  INTERFACE
    $<$<CXX_COMPILER_ID:GNU,Clang,AppleClang>:-Wall;-Wextra;-Wpedantic;-Wshadow;-Wconversion;-Wsign-conversion>
)

configure_file(src/hz/version.hpp.in include/hz/version.hpp @ONLY)

# hz_core: everything except argument parsing and output formatting.
add_library(hz_core STATIC src/hz/ulid.cpp)
target_include_directories(hz_core PUBLIC src ${CMAKE_CURRENT_BINARY_DIR}/include)
target_link_libraries(
  hz_core
  PUBLIC tomlplusplus::tomlplusplus nlohmann_json::nlohmann_json SQLite::SQLite3
  PRIVATE hz_warnings)

add_executable(hz src/cli/main.cpp)
target_link_libraries(hz PRIVATE hz_core CLI11::CLI11 hz_warnings)

install(TARGETS hz)

if(HZ_BUILD_TESTS)
  enable_testing()
  add_subdirectory(tests)
endif()
