find_package(Catch2 3 CONFIG REQUIRED)
include(Catch)

add_executable(hz_unit_tests unit/ulid_test.cpp)
target_link_libraries(hz_unit_tests PRIVATE hz_core Catch2::Catch2WithMain hz_warnings)
catch_discover_tests(hz_unit_tests)

# Black-box checks against the built binary.
add_test(NAME cli.version COMMAND hz --version)
set_tests_properties(cli.version PROPERTIES PASS_REGULAR_EXPRESSION "^hz ${PROJECT_VERSION}\n$")

add_test(NAME cli.help COMMAND hz)
set_tests_properties(cli.help PROPERTIES PASS_REGULAR_EXPRESSION "--version")
