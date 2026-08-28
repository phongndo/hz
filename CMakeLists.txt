cmake_minimum_required(VERSION 3.30)

project(hz VERSION 0.1.0 LANGUAGES CXX)

if(NOT CMAKE_CXX_COMPILER_ID MATCHES "^(AppleClang|Clang)$")
  message(FATAL_ERROR "hz requires Clang or Apple Clang")
endif()

set(CMAKE_CXX_EXTENSIONS OFF)
set(CMAKE_CXX_STANDARD 23)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_EXPORT_COMPILE_COMMANDS ON)

option(HZ_BUILD_TESTS "Build the test suite" ON)
option(HZ_BUILD_BENCHMARKS "Build benchmarks" ON)

add_library(hz_warnings INTERFACE)
target_compile_options(
  hz_warnings
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
  target_compile_options(hz_warnings INTERFACE -Wno-unused-command-line-argument)
  add_link_options("LINKER:-z,noexecstack")
endif()

set(HZ_GENERATED_INCLUDE_DIR "${CMAKE_CURRENT_BINARY_DIR}/generated/include")
configure_file(
  "${CMAKE_CURRENT_SOURCE_DIR}/include/hz/version.hpp.in"
  "${HZ_GENERATED_INCLUDE_DIR}/hz/version.hpp"
  @ONLY
)

function(hz_internal_target target)
  target_include_directories(
    ${target}
    PUBLIC
      "$<BUILD_INTERFACE:${CMAKE_CURRENT_SOURCE_DIR}/src>"
      "$<BUILD_INTERFACE:${HZ_GENERATED_INCLUDE_DIR}>"
  )
  target_link_libraries(${target} PRIVATE hz_warnings)
endfunction()

add_library(hz_app STATIC src/app/application.cpp)
hz_internal_target(hz_app)

add_executable(hz apps/hz/main.cpp)
target_link_libraries(hz PRIVATE hz_app hz_warnings)
install(TARGETS hz RUNTIME DESTINATION bin)

if(HZ_BUILD_TESTS)
  enable_testing()
  find_package(GTest CONFIG REQUIRED)
  include(GoogleTest)

  add_executable(hz_unit_tests tests/application_test.cpp)
  hz_internal_target(hz_unit_tests)
  target_link_libraries(hz_unit_tests PRIVATE hz_app GTest::gtest_main)
  gtest_discover_tests(hz_unit_tests PROPERTIES LABELS "unit")

  add_test(NAME cli.help COMMAND hz --help)
  set_tests_properties(cli.help PROPERTIES LABELS "integration")
endif()

if(HZ_BUILD_BENCHMARKS)
  find_package(benchmark CONFIG REQUIRED)

  add_executable(hz_benchmarks benchmarks/hz_benchmark.cpp)
  hz_internal_target(hz_benchmarks)
  target_link_libraries(hz_benchmarks PRIVATE hz_app benchmark::benchmark)
endif()
