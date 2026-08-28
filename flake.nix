{
  description = "hz C++23 development environment";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    hk.url = "github:jdx/hk/v1.50.0";
  };

  outputs =
    { self, nixpkgs, hk, ... }:
    let
      systems = [
        "aarch64-darwin"
        "aarch64-linux"
        "x86_64-darwin"
        "x86_64-linux"
      ];
      forAllSystems = nixpkgs.lib.genAttrs systems;
    in
    {
      packages = forAllSystems (
        system:
        let
          pkgs = import nixpkgs { inherit system; };
          llvm = pkgs.llvmPackages_22;
          mkHz = buildType:
            llvm.stdenv.mkDerivation {
              pname = "hz";
              version = "0.1.0";
              src = self;

              nativeBuildInputs = [
                pkgs.cmake
                pkgs.ninja
              ];

              cmakeFlags = [
                "-DCMAKE_BUILD_TYPE=${buildType}"
                "-DCMAKE_CXX_SCAN_FOR_MODULES=OFF"
                "-DHZ_BUILD_TESTS=OFF"
                "-DHZ_BUILD_BENCHMARKS=OFF"
              ];

              doInstallCheck = true;
              installCheckPhase = ''
                "$out/bin/hz" --version | grep -q '^hz '
              '';

              meta = {
                description = "Fast, independent development workspaces";
                homepage = "https://github.com/phongndo/hz";
                license = pkgs.lib.licenses.mit;
                mainProgram = "hz";
                platforms = systems;
              };
            };
        in
        rec {
          hz = mkHz "Release";
          default = hz;
        }
      );

      apps = forAllSystems (
        system:
        rec {
          hz = {
            type = "app";
            program = "${self.packages.${system}.hz}/bin/hz";
          };
          default = hz;
        }
      );

      devShells = forAllSystems (
        system:
        let
          pkgs = import nixpkgs { inherit system; };
          llvm = pkgs.llvmPackages_22;
          isDarwin = pkgs.stdenv.hostPlatform.isDarwin;
          darwinTools = llvm.clang-tools;
          devHz = pkgs.writeShellScriptBin "hz" ''
            root="$(${pkgs.git}/bin/git rev-parse --show-toplevel)" || {
              echo "hz: not inside an hz checkout" >&2
              exit 1
            }
            exec "$root/scripts/dev-run" "$@"
          '';
          linuxClangd = pkgs.writeShellScriptBin "clangd" ''
            exec "${llvm.clang-tools}/bin/clangd" \
              --query-driver="${llvm.clang}/bin/clang++,${llvm.clang}/bin/clang" \
              "$@"
          '';
          linuxClangTidy = pkgs.writeShellScriptBin "clang-tidy" ''
            exec "${llvm.clang-tools}/bin/clang-tidy" \
              --extra-arg-before=-resource-dir \
              --extra-arg-before="${llvm.clang}/resource-root" \
              --extra-arg-before=-isystem \
              --extra-arg-before="${pkgs.gcc.cc}/include/c++/${pkgs.gcc.version}" \
              --extra-arg-before=-isystem \
              --extra-arg-before="${pkgs.gcc.cc}/include/c++/${pkgs.gcc.version}/${pkgs.stdenv.hostPlatform.config}" \
              --extra-arg-before=-idirafter \
              --extra-arg-before="${llvm.stdenv.cc.libc_dev}/include" \
              "$@"
          '';
          darwinClang = pkgs.writeShellScriptBin "clang" ''
            exec /usr/bin/clang "$@"
          '';
          darwinClangxx = pkgs.writeShellScriptBin "clang++" ''
            exec /usr/bin/clang++ "$@"
          '';
          darwinClangd = pkgs.writeShellScriptBin "clangd" ''
            resource_dir="$(/usr/bin/env -u SDKROOT DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer /usr/bin/xcrun clang -print-resource-dir)" || exit 1
            exec "${darwinTools}/bin/clangd-unwrapped" \
              --resource-dir="$resource_dir" \
              "$@"
          '';
          darwinClangFormat = pkgs.writeShellScriptBin "clang-format" ''
            exec "${darwinTools}/bin/clang-format" "$@"
          '';
          darwinClangTidy = pkgs.writeShellScriptBin "clang-tidy" ''
            sdk="$(/usr/bin/env -u SDKROOT DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer /usr/bin/xcrun --sdk macosx --show-sdk-path)" || exit 1
            resource_dir="$(/usr/bin/env -u SDKROOT DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer /usr/bin/xcrun clang -print-resource-dir)" || exit 1
            exec "${darwinTools}/bin/clang-tidy-unwrapped" \
              --extra-arg-before=-isysroot \
              --extra-arg-before="$sdk" \
              --extra-arg-before=-resource-dir \
              --extra-arg-before="$resource_dir" \
              "$@"
          '';
          compilerPackages = pkgs.lib.optionals isDarwin [
            darwinClang
            darwinClangxx
            darwinClangd
            darwinClangFormat
            darwinClangTidy
          ] ++ pkgs.lib.optionals (!isDarwin) [
            llvm.clang
            llvm.clang-tools
            linuxClangd
            linuxClangTidy
          ];
          projectPackages = compilerPackages ++ [
            devHz
            pkgs.ccache
            pkgs.cmake
            pkgs.conan
            pkgs.git
            pkgs.just
            pkgs.ninja
            pkgs.nixd
            pkgs.nixpkgs-fmt
            pkgs.pkg-config
          ];
          qualityPackages = [
            pkgs.actionlint
            pkgs.shellcheck
            hk.packages.${system}.default
          ];
          shellEnvironment = {
            CMAKE_GENERATOR = "Ninja";
            shellHook =
              if isDarwin then ''
                export PATH="${darwinClang}/bin:${darwinClangxx}/bin:${darwinClangd}/bin:${darwinClangFormat}/bin:${darwinClangTidy}/bin:$PATH"
                export CC=/usr/bin/clang
                export CXX=/usr/bin/clang++
                export DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer
                export SDKROOT="$(/usr/bin/xcrun --sdk macosx --show-sdk-path)"
              '' else ''
                export PATH="${linuxClangd}/bin:${linuxClangTidy}/bin:$PATH"
                export CC="${llvm.clang}/bin/clang"
                export CXX="${llvm.clang}/bin/clang++"
              '';
          };
        in
        {
          default = pkgs.mkShell (
            shellEnvironment
            // {
              packages = projectPackages ++ qualityPackages;
            }
          );

          platform = pkgs.mkShell (
            shellEnvironment
            // {
              packages = projectPackages;
            }
          );

          ci = pkgs.mkShell (
            shellEnvironment
            // {
              packages = projectPackages ++ qualityPackages;
            }
          );
        }
      );

      formatter = forAllSystems (
        system:
        let
          pkgs = import nixpkgs { inherit system; };
        in
        pkgs.writeShellApplication {
          name = "format-flake";
          runtimeInputs = [ pkgs.nixpkgs-fmt ];
          text = ''exec nixpkgs-fmt "$PWD/flake.nix"'';
        }
      );
    };
}
