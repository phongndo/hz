{
  lib,
  stdenv,
  cmake,
  ninja,
  cli11,
  tomlplusplus,
  nlohmann_json,
  sqlite,
  catch2_3,
  git,
  bash,
  zsh,
  fish,
}:

stdenv.mkDerivation {
  pname = "hz";
  # CMakeLists.txt is the single home of the version number.
  version = builtins.head (
    builtins.match ".*project\\([[:space:]]*hz[[:space:]]+VERSION[[:space:]]+([0-9.]+).*" (
      builtins.readFile ../CMakeLists.txt
    )
  );

  src = lib.fileset.toSource {
    root = ../.;
    fileset = lib.fileset.unions [
      ../CMakeLists.txt
      ../src
      ../shell
      ../tests
    ];
  };

  nativeBuildInputs = [
    cmake
    ninja
  ];
  buildInputs = [
    cli11
    tomlplusplus
    nlohmann_json
    sqlite
    catch2_3
  ];

  # The test suite drives real git repositories.
  nativeCheckInputs = [
    git
    bash
    zsh
    fish
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
    license = lib.licenses.mit;
    mainProgram = "hz";
  };
}
