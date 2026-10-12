#include "hz/installation.hpp"

#include <catch2/catch_test_macros.hpp>

#include <string>

TEST_CASE("the install script's receipt says where it installed hz", "[installation]") {
    const auto found = hz::parse_installation("release=v0.10.0\n"
                                              "install_dir=/home/me/.local/bin\n"
                                              "releases_dir=/home/me/.local/share/hz/releases\n");
    REQUIRE(found);
    REQUIRE(found->install_dir == "/home/me/.local/bin");
    REQUIRE(found->releases_dir == "/home/me/.local/share/hz/releases");

    // Paths keep their spaces and any '=' after the first.
    const auto spaced = hz::parse_installation("install_dir=/a b/bin\nreleases_dir=/a=b\n");
    REQUIRE(spaced);
    REQUIRE(spaced->install_dir == "/a b/bin");
    REQUIRE(spaced->releases_dir == "/a=b");

    REQUIRE_FALSE(hz::parse_installation(""));
    REQUIRE_FALSE(hz::parse_installation("install_dir=/bin\n"));                    // no releases
    REQUIRE_FALSE(hz::parse_installation("install_dir=bin\nreleases_dir=/data\n")); // relative
}

TEST_CASE("the executable's path says what installed hz", "[installation]") {
    using hz::Installer;
    const auto macos = hz::installed_by("/opt/homebrew/Cellar/hz-cli/1.2.3/bin/hz", "", false);
    REQUIRE(macos.installer == Installer::homebrew);
    REQUIRE(macos.homebrew_prefix == "/opt/homebrew");
    REQUIRE(macos.homebrew_formula == "hz-cli");
    const auto linuxbrew =
        hz::installed_by("/home/linuxbrew/.linuxbrew/Cellar/hz-cli/1.2.3/bin/hz", "", false);
    REQUIRE(linuxbrew.installer == Installer::homebrew);
    REQUIRE(linuxbrew.homebrew_prefix == "/home/linuxbrew/.linuxbrew");
    REQUIRE(linuxbrew.homebrew_formula == "hz-cli");

    const std::string mise = "/home/me/.local/share/mise/installs/github-phongndo-hz/1.2.3/bin/hz";
    REQUIRE(hz::installed_by(mise, "", false).installer == Installer::mise);
    const std::string moved = "/opt/mise/installs/github-phongndo-hz/1.2.3/bin/hz";
    REQUIRE(hz::installed_by(moved, "/opt/mise", false).installer == Installer::mise);
    REQUIRE(hz::installed_by(moved, "/opt/mise/", false).installer == Installer::mise);
    // With MISE_DATA_DIR set, mise installs nothing anywhere else.
    REQUIRE(hz::installed_by(mise, "/opt/mise", false).installer == Installer::other);
    REQUIRE(
        hz::installed_by("/opt/mise-old/installs/hz/1.2.3/bin/hz", "/opt/mise", false).installer ==
        Installer::other);

    const auto nix = hz::installed_by("/nix/store/0123abcd-hz-0.10.0/bin/hz", "", false);
    REQUIRE(nix.installer == Installer::nix);
    REQUIRE(nix.nix_store_path == "/nix/store/0123abcd-hz-0.10.0");

    REQUIRE(hz::installed_by("/home/me/src/hz/build/hz", "", false).installer == Installer::other);
    REQUIRE(hz::installed_by("", "", false).installer == Installer::other);

    // The install script's receipt wins over the path.
    REQUIRE(
        hz::installed_by("/home/me/.local/share/hz/releases/v1.2.3-x/bin/hz", "", true).installer ==
        Installer::install_script);
    REQUIRE(hz::installed_by(mise, "", true).installer == Installer::install_script);
}

TEST_CASE("the Nix profile names the element holding hz", "[installation]") {
    // `nix profile list --json` from Nix 2.35 after `nix profile add` of nixpkgs#hello and of an hz
    // checkout in a directory named hz-wt-update, which names the element.
    const std::string listed =
        R"({"elements":{"hello":{"active":true,"attrPath":"legacyPackages.x86_64-linux.hello",)"
        R"("originalUrl":"github:NixOS/nixpkgs/a7868a727837f3c09cee2ce0ca671c76b1589fed",)"
        R"("outputs":null,"priority":5,"storePaths":["/nix/store/)"
        R"(5z2yp3ysx8476c8g5w25b0smlgkjvaq3-hello-2.12.3"],)"
        R"("url":"github:NixOS/nixpkgs/a7868a727837f3c09cee2ce0ca671c76b1589fed"},)"
        R"("hz-wt-update":{"active":true,"attrPath":"packages.x86_64-linux.default",)"
        R"("originalUrl":"git+file:///home/dp/code/hz-wt-update","outputs":null,"priority":5,)"
        R"("storePaths":["/nix/store/ripb2qp7gdkj48armv9r5safz4wch0lv-hz-0.10.0"],)"
        R"("url":"git+file:///home/dp/code/hz-wt-update?ref=refs/heads/update-any-install)"
        R"(&rev=b8d9dad97a95accc60021799d512067638947f6f"}},"version":3})";
    REQUIRE(hz::nix_profile_element(
                listed, "/nix/store/ripb2qp7gdkj48armv9r5safz4wch0lv-hz-0.10.0") == "hz-wt-update");
    REQUIRE_FALSE(hz::nix_profile_element(listed, "/nix/store/0123abcd-hz-0.10.0"));

    REQUIRE_FALSE(hz::nix_profile_element("{\"elements\":", "/nix/store/0123abcd-hz-0.10.0"));
    REQUIRE_FALSE(hz::nix_profile_element("", "/nix/store/0123abcd-hz-0.10.0"));
    // Before version 3 the elements were an array, without names.
    REQUIRE_FALSE(hz::nix_profile_element(
        R"({"version":2,"elements":[{"storePaths":["/nix/store/0123abcd-hz-0.10.0"]}]})",
        "/nix/store/0123abcd-hz-0.10.0"));
}
