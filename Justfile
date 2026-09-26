sources := `git ls-files '*.cpp' '*.hpp' | tr '\n' ' '`

# Configure the debug build (run once, or after editing CMake files)
setup:
    cmake --preset debug

build:
    [ -f build/debug/build.ninja ] || cmake --preset debug
    cmake --build --preset debug

test: build
    ctest --preset debug

release:
    cmake --preset release
    cmake --build --preset release

check: fmt-check tidy

fmt:
    clang-format -i {{sources}}

fmt-check:
    clang-format --dry-run --Werror {{sources}}

tidy:
    run-clang-tidy -p build/debug -j 4 -warnings-as-errors='*'

# Build and run the debug binary
hz *args: build
    ./build/debug/hz {{args}}

clean:
    rm -rf build

# Run hk checks (equivalent to pre-commit hook steps)
hk-check:
    mise x hk -- hk check

hooks:
    mise x hk -- hk validate
