find_package(Catch2 3 CONFIG REQUIRED)
include(Catch)

add_executable(hz_unit_tests unit/ulid_test.cpp unit/filter_test.cpp unit/tree_test.cpp
                             unit/workspaces_test.cpp unit/lifecycle_test.cpp unit/process_test.cpp)
target_link_libraries(hz_unit_tests PRIVATE hz_core Catch2::Catch2WithMain hz_warnings)
catch_discover_tests(hz_unit_tests)

# Black-box tests: spawn the built hz binary with a private registry.
add_executable(hz_cli_tests cli/identity_test.cpp cli/lifecycle_test.cpp cli/git_test.cpp cli/shell_test.cpp)
target_compile_definitions(hz_cli_tests PRIVATE HZ_BINARY="$<TARGET_FILE:hz>")
target_link_libraries(hz_cli_tests PRIVATE hz_core Catch2::Catch2WithMain hz_warnings)
add_dependencies(hz_cli_tests hz)
catch_discover_tests(hz_cli_tests TEST_PREFIX "cli: ")

add_test(NAME cli.version COMMAND hz --version)
set_tests_properties(cli.version PROPERTIES PASS_REGULAR_EXPRESSION "^hz ${PROJECT_VERSION}\n$")
