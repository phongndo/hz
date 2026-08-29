from conan import ConanFile


class LightConan(ConanFile):
    package_type = "application"
    settings = "os", "arch", "compiler", "build_type"

    requires = (
        "benchmark/1.9.5",
        "gtest/1.17.0",
    )

    generators = "CMakeDeps", "CMakeToolchain"
