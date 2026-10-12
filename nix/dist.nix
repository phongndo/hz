# The release archive for one platform: an hz that runs without Nix, the installer that
# `hz update` reruns, and the licenses of the libraries linked into it. On Linux the executable
# is static (musl), so it runs on any distribution. On macOS nixpkgs already links the system's
# libc++, so building toml++ and SQLite static leaves only macOS's own libraries.
{ pkgs }:

let
  inherit (pkgs) lib;
  inherit (pkgs.stdenv) hostPlatform;

  target =
    {
      x86_64-linux = "x86_64-unknown-linux-musl";
      aarch64-linux = "aarch64-unknown-linux-musl";
      aarch64-darwin = "aarch64-apple-darwin";
    }
    .${hostPlatform.system};

  deps =
    if hostPlatform.isLinux then
      {
        inherit (pkgs.pkgsStatic)
          cli11
          tomlplusplus
          nlohmann_json
          sqlite
          ;
        musl = pkgs.pkgsStatic.stdenv.cc.libc;
      }
    else
      let
        static = pkg: pkg.override { stdenv = pkgs.stdenvAdapters.makeStaticLibraries pkgs.stdenv; };
      in
      {
        inherit (pkgs) cli11 nlohmann_json;
        # nixpkgs builds toml++ as a compiled library, not only headers.
        tomlplusplus = static pkgs.tomlplusplus;
        sqlite = static pkgs.sqlite;
      };

  hz =
    if hostPlatform.isLinux then
      (pkgs.pkgsStatic.callPackage ./package.nix { }).overrideAttrs (old: {
        cmakeFlags = old.cmakeFlags ++ [
          # musl gives threads 128 KiB of stack unless the executable asks for more; this is
          # glibc's 8 MiB.
          "-DCMAKE_EXE_LINKER_FLAGS=-Wl,-z,stack-size=8388608"
        ];
      })
    else
      pkgs.callPackage ./package.nix {
        stdenv = pkgs.clangStdenv;
        inherit (deps) tomlplusplus sqlite;
      };

  # Sources whose licenses ask for their notice in binary distributions. (SQLite is in the
  # public domain; libstdc++ and libgcc carry the GCC Runtime Library Exception.)
  notices = {
    CLI11 = deps.cli11.src;
    "toml++" = deps.tomlplusplus.src;
    nlohmann_json = deps.nlohmann_json.src;
  }
  // lib.optionalAttrs hostPlatform.isLinux { musl = deps.musl.src; };
in
pkgs.stdenvNoCC.mkDerivation {
  pname = "hz-dist";
  inherit (hz) version;
  dontUnpack = true;
  nativeBuildInputs = [ pkgs.llvmPackages.llvm ]; # llvm-readelf, llvm-otool

  buildPhase = ''
    runHook preBuild
    name=hz-v$version-${target}
    root=$PWD/$name
    install -Dm755 ${hz}/bin/hz $root/bin/hz
    install -Dm755 ${../scripts/install.sh} $root/share/hz/install.sh
    install -Dm644 ${../LICENSE} $root/LICENSE
    install -Dm644 ${../README.md} $root/README.md

    ${lib.concatStrings (
      lib.mapAttrsToList (name: src: ''
        unpacked=$TMPDIR/notices/${lib.escapeShellArg name}
        mkdir -p $unpacked
        if [ -d ${src} ]; then cp -r ${src}/. $unpacked; else tar -xf ${src} -C $unpacked; fi
        top=$(find $unpacked -mindepth 1 -maxdepth 1)
        [ -d "$top" ] && [ "$(printf '%s\n' "$top" | wc -l)" -eq 1 ] || top=$unpacked
        found=
        for file in "$top"/LICEN[CS]E* "$top"/COPYING* "$top"/COPYRIGHT*; do
          [ -f "$file" ] || continue
          install -Dm644 "$file" "$root/share/doc/hz/licenses/"${lib.escapeShellArg name}"/''${file##*/}"
          found=1
        done
        [ -n "$found" ] || { echo "no license file in ${src}" >&2; exit 1; }
      '') notices
    )}

    bin=$root/bin/hz
    [ "$($bin --version)" = "hz $version" ]
    ${
      if hostPlatform.isLinux then
        ''
          if llvm-readelf --program-headers $bin | grep -q INTERP || llvm-readelf --dynamic $bin | grep -q NEEDED; then
            echo "hz is not statically linked" >&2
            exit 1
          fi
        ''
      else
        ''
          # After the first line, otool -L lists the libraries the executable loads.
          if llvm-otool -L $bin | tail -n +2 | grep -vE '^[[:space:]]+(/usr/lib/|/System/Library/)'; then
            echo "hz links libraries that macOS does not have" >&2
            exit 1
          fi
        ''
    }

    mkdir -p $out
    tar --sort=name --mtime=@$SOURCE_DATE_EPOCH --owner=0 --group=0 --numeric-owner -cf - $name |
      gzip -9n >$out/$name.tar.gz
    (cd $out && sha256sum $name.tar.gz >$name.tar.gz.sha256)
    runHook postBuild
  '';

  dontInstall = true;

  passthru = { inherit hz target; };
}
