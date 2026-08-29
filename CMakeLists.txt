cmake_minimum_required(VERSION 3.30)

project(light VERSION 0.1.0 LANGUAGES CXX)

if(NOT CMAKE_CXX_COMPILER_ID MATCHES "^(AppleClang|Clang)$")
  message(FATAL_ERROR "light requires Clang or Apple Clang")
endif()

set(CMAKE_CXX_EXTENSIONS OFF)
set(CMAKE_CXX_STANDARD 23)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_EXPORT_COMPILE_COMMANDS ON)

option(LIGHT_BUILD_TESTS "Build the test suite" ON)
option(LIGHT_BUILD_BENCHMARKS "Build benchmarks" ON)

add_library(light_warnings INTERFACE)
target_compile_options(
  light_warnings
  INTERFACE
    -Wall
    -Wextra
    -Wpedantic
    -Werror
    -Wcast-align
    -Wcast-qual
    -Wconversion
    -Wdocumentation
    -Wdouble-promotion
    -Wextra-semi
    -Wformat=2
    -Wimplicit-fallthrough
    -Wmissing-declarations
    -Wnon-virtual-dtor
    -Wnull-dereference
    -Wold-style-cast
    -Woverloaded-virtual
    -Wshadow-all
    -Wsign-conversion
    -Wswitch-enum
    -Wthread-safety
    -Wundef
    -Wunreachable-code-aggressive
    -Wzero-as-null-pointer-constant
)

if(CMAKE_SYSTEM_NAME STREQUAL "Linux")
  target_compile_options(light_warnings INTERFACE -Wno-unused-command-line-argument)
  add_link_options("LINKER:-z,noexecstack")
endif()

set(LIGHT_GENERATED_INCLUDE_DIR "${CMAKE_CURRENT_BINARY_DIR}/generated/include")
configure_file(
  "${CMAKE_CURRENT_SOURCE_DIR}/include/light/version.hpp.in"
  "${LIGHT_GENERATED_INCLUDE_DIR}/light/version.hpp"
  @ONLY
)

function(light_internal_target target)
  target_include_directories(
    ${target}
    PUBLIC
      "$<BUILD_INTERFACE:${CMAKE_CURRENT_SOURCE_DIR}/src>"
      "$<BUILD_INTERFACE:${LIGHT_GENERATED_INCLUDE_DIR}>"
  )
  target_link_libraries(${target} PRIVATE light_warnings)
endfunction()

add_library(light_app STATIC src/app/application.cpp)
light_internal_target(light_app)

add_executable(light apps/light/main.cpp)
target_link_libraries(light PRIVATE light_app light_warnings)
install(TARGETS light RUNTIME DESTINATION bin)

if(LIGHT_BUILD_TESTS)
  enable_testing()
  find_package(GTest CONFIG REQUIRED)
  include(GoogleTest)

  add_executable(light_unit_tests tests/application_test.cpp)
  light_internal_target(light_unit_tests)
  target_link_libraries(light_unit_tests PRIVATE light_app GTest::gtest_main)
  gtest_discover_tests(light_unit_tests PROPERTIES LABELS "unit")

  add_test(NAME cli.help COMMAND light --help)
  set_tests_properties(cli.help PROPERTIES LABELS "integration")
endif()

if(LIGHT_BUILD_BENCHMARKS)
  find_package(benchmark CONFIG REQUIRED)

  add_executable(light_benchmarks benchmarks/light_benchmark.cpp)
  light_internal_target(light_benchmarks)
  target_link_libraries(light_benchmarks PRIVATE light_app benchmark::benchmark)
endif()
