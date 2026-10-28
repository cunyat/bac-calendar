{
  lib,
  rustPlatform,
}: let
  manifest = (lib.importTOML ../Cargo.toml).package;
in
  rustPlatform.buildRustPackage {
    pname = manifest.name;
    inherit (manifest) version;

    src = lib.fileset.toSource {
      root = ../.;
      fileset = lib.fileset.unions [
        ../Cargo.toml
        ../Cargo.lock
        ../src
        ../tests
      ];
    };

    cargoLock.lockFile = ../Cargo.lock;

    meta = {
      inherit (manifest) description;
      license = lib.licenses.mit;
      mainProgram = "bac-calendar";
      platforms = lib.platforms.linux;
    };
  }
