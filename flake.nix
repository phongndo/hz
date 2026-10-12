{
  description = "hz: copy-on-write workspaces for parallel humans and agents";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs }:
    let
      # nixpkgs 26.11 dropped x86_64-darwin.
      systems = [
        "aarch64-darwin"
        "aarch64-linux"
        "x86_64-linux"
      ];

      forAllSystems =
        function: nixpkgs.lib.genAttrs systems (system: function nixpkgs.legacyPackages.${system});
    in
    {
      packages = forAllSystems (pkgs: {
        default = pkgs.callPackage ./nix/package.nix { stdenv = pkgs.clangStdenv; };
        # This system's release archive and its checksum, in ./result
        dist = import ./nix/dist.nix { inherit pkgs; };
      });

      checks = forAllSystems (pkgs: {
        package = self.packages.${pkgs.stdenv.hostPlatform.system}.default;

        actionlint =
          pkgs.runCommand "hz-actionlint"
            {
              nativeBuildInputs = [
                pkgs.actionlint
                pkgs.shellcheck
              ];
            }
            ''
              cd ${self} && actionlint .github/workflows/*.yml
              touch $out
            '';

        shellcheck = pkgs.runCommand "hz-shellcheck" { nativeBuildInputs = [ pkgs.shellcheck ]; } ''
          shellcheck ${self}/scripts/install.sh
          touch $out
        '';
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
            pkgs.hk
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
