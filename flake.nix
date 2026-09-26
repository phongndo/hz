{
  description = "hz: copy-on-write workspaces for parallel humans and agents";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs }:
    let
      systems = [
        "aarch64-darwin"
        "aarch64-linux"
        "x86_64-darwin"
        "x86_64-linux"
      ];

      forAllSystems =
        function: nixpkgs.lib.genAttrs systems (system: function nixpkgs.legacyPackages.${system});

      # CMakeLists.txt is the single home of the version number.
      version = builtins.head (
        builtins.match ".*project\\([[:space:]]*hz[[:space:]]+VERSION[[:space:]]+([0-9.]+).*" (
          builtins.readFile ./CMakeLists.txt
        )
      );

      dependencies = pkgs: [
        pkgs.cli11
        pkgs.tomlplusplus
        pkgs.nlohmann_json
        pkgs.sqlite
        pkgs.catch2_3
      ];
    in
    {
      packages = forAllSystems (pkgs: {
        default = pkgs.clangStdenv.mkDerivation {
          pname = "hz";
          inherit version;
          src = self;

          nativeBuildInputs = [
            pkgs.cmake
            pkgs.ninja
          ];
          buildInputs = dependencies pkgs;

          # The test suite drives real git repositories.
          nativeCheckInputs = [
            pkgs.git
            pkgs.bash
            pkgs.zsh
            pkgs.fish
          ];
          cmakeFlags = [ "-DHZ_BUILD_TESTS=ON" ];
          doCheck = true;
          checkPhase = ''
            runHook preCheck
            ctest --output-on-failure
            runHook postCheck
          '';

          meta = {
            description = "Copy-on-write workspaces for parallel humans and agents";
            license = pkgs.lib.licenses.mit;
            mainProgram = "hz";
          };
        };
      });

      checks = forAllSystems (pkgs: {
        package = self.packages.${pkgs.stdenv.hostPlatform.system}.default;
      });

      devShells = forAllSystems (pkgs: {
        benchmark = pkgs.mkShell.override { stdenv = pkgs.clangStdenv; } {
          inputsFrom = [ self.devShells.${pkgs.stdenv.hostPlatform.system}.default ];
          packages = [
            pkgs.cargo
            pkgs.rustc
            pkgs.pkg-config
            pkgs.time
            pkgs.nodejs_22
          ];
        };

        default = pkgs.mkShell.override { stdenv = pkgs.clangStdenv; } {
          inputsFrom = [ self.packages.${pkgs.stdenv.hostPlatform.system}.default ];
          packages = [
            pkgs.clang-tools # clangd, clang-format, clang-tidy
            pkgs.python3 # run-clang-tidy
            pkgs.git
            pkgs.just
            pkgs.nixd
          ];
          shellHook = ''
            export PATH="$PWD/build/debug:$PATH"
          '';
        };
      });

      formatter = forAllSystems (pkgs: pkgs.nixfmt);
    };
}
