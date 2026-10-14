# andon: the signal board for many agents on one repository.
# Flake-free; consume with `callPackage ./path/to/andon/package.nix { }`.
{ lib, rustPlatform }:

let
  # The one version source is Cargo.toml: `andon --version`, this package, the
  # flake and the release tooling all read it from there.
  version = (builtins.fromTOML (builtins.readFile ./Cargo.toml)).package.version;
in
rustPlatform.buildRustPackage {
  pname = "andon";
  inherit version;

  # An allowlist: .design/ and the living documents do not change
  # the package. Add a path here when the build or its tests start reading it.
  src = lib.fileset.toSource {
    root = ./.;
    fileset = lib.fileset.unions [
      ./Cargo.toml
      ./Cargo.lock
      ./src
      ./tests
    ];
  };

  cargoLock.lockFile = ./Cargo.lock;

  doCheck = true;

  doInstallCheck = true;
  installCheckPhase = ''
    runHook preInstallCheck
    [ "$($out/bin/andon --version)" = "andon ${version}" ]
    $out/bin/andon --help >/dev/null
    runHook postInstallCheck
  '';

  meta = {
    description = "The signal board for many agents on one repository, derived from artifacts";
    mainProgram = "andon";
    license = lib.licenses.mit;
    platforms = lib.platforms.unix;
  };
}
