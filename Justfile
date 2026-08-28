set positional-arguments

profile := "debug"
build_type := if profile == "release" { "Release" } else if profile == "dev" { "Dev" } else { "Debug" }
conan_build_type := if profile == "dev" { "Release" } else { build_type }
cpp_roots := "apps include src tests benchmarks"

_default:
    @just --list

# Show the pinned development tool versions.
versions:
    clang++ --version
    clangd --version
    clang-tidy --version
    cmake --version
    ninja --version
    ccache --version
    conan --version
    hk --version

# Install Conan dependencies for the selected profile.
deps:
    rm -f CMakeUserPresets.json
    conan install . \
        --output-folder=build/{{ profile }}/conan \
        --lockfile=conan.lock \
        --profile:all=conan/profiles/llvm \
        --settings=build_type={{ conan_build_type }} \
        --conf=tools.cmake.cmaketoolchain:user_presets= \
        --build=missing

# Generate Ninja files and compile_commands.json.
configure: deps
    cmake --preset {{ profile }} \
        -DCMAKE_BUILD_TYPE={{ build_type }} \
        -DCMAKE_TOOLCHAIN_FILE="$PWD/build/{{ profile }}/conan/conan_toolchain.cmake" \
        -DHZ_BUILD_TESTS=ON -DHZ_BUILD_BENCHMARKS=ON

# Build the application, tests, and benchmarks.
build: configure
    cmake --build --preset {{ profile }}

# Incrementally build and run this checkout's hz binary.
run *args:
    @exec ./scripts/dev-run "$@"

# Build and run all tests.
test: build
    scripts/ci/test {{ profile }}

# Build and run the release benchmarks.
bench:
    scripts/ci/configure release -DHZ_BUILD_TESTS=OFF -DHZ_BUILD_BENCHMARKS=ON
    cmake --build --preset release --target hz_benchmarks
    ./build/release/hz_benchmarks

# Format C++ and Nix files in place.
fmt:
    bash -c "find {{ cpp_roots }} -type f \
        \( -name '*.cpp' -o -name '*.hpp' -o -name '*.hpp.in' \) -print0 | \
        xargs -0 clang-format -i"
    nixpkgs-fmt flake.nix

# Check formatting without changing files.
fmt-check:
    scripts/ci/format

# Run responsive clang-tidy checks in parallel.
lint: configure
    bash -c "find apps src tests benchmarks -type f -name '*.cpp' -print0 | \
        xargs -0 -n 1 -P \"\${CLANG_TIDY_JOBS:-4}\" \
        clang-tidy --quiet -p build/{{ profile }}"

# Check every production source and header through clangd.
lsp-check: configure
    bash -c "find apps src -type f \
        \( -name '*.hpp' -o -name '*.cpp' \) -print0 | sort -z | \
        xargs -0 -n 1 -P \"\${CLANGD_JOBS:-4}\" cmake/check-clangd.sh"

# Start clangd for editor integrations.
lsp:
    clangd --enable-config

# Run formatting, build, tests, clang-tidy, and clangd diagnostics.
check: build fmt-check lint lsp-check test

# Reproduce the merge-blocking build and test lane.
ci-build-test:
    scripts/ci/build-test

# Reproduce the merge-blocking clang-tidy lane.
ci-lint:
    scripts/ci/lint

# Reproduce the merge-blocking clangd lane.
ci-lsp:
    scripts/ci/lsp

# Reproduce the sanitizer lane.
ci-sanitizers:
    scripts/ci/sanitizers

# Check CI shell scripts and GitHub Actions workflows.
ci-workflows:
    scripts/ci/workflows

# Reproduce every merge-blocking CI lane locally.
ci-check:
    scripts/ci/format
    scripts/ci/build-test
    scripts/ci/lint
    scripts/ci/lsp
    scripts/ci/sanitizers
    scripts/ci/workflows

# Configure the debug tree and install repository hooks.
hooks:
    scripts/ci/configure debug -DHZ_BUILD_TESTS=ON -DHZ_BUILD_BENCHMARKS=ON
    hk validate
    hk install

# Run every configured hook check.
hooks-check:
    hk check --all --check

# Apply safe pre-commit fixes.
hooks-fix:
    hk fix --all

# Remove generated build output.
clean:
    rm -rf build CMakeUserPresets.json
