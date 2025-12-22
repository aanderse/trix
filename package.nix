{
  lib,
  rustPlatform,
  versionCheckHook,
}:

rustPlatform.buildRustPackage {
  pname = "trix";
  version = "0.2.0";

  src = lib.fileset.toSource {
    root = ./.;
    fileset = lib.fileset.unions [
      ./Cargo.toml
      ./Cargo.lock
      ./tests
      ./src
      ./direnvrc
    ];
  };

  cargoHash = "sha256-VCAfIvHR9Z4yXb7BwuaIQt6r6jP0RTxyhSKrQo54nhw=";

  dontUseCargoParallelTests = true;

  doCheck = false;

  postInstall = ''
    mkdir -p $out/share/trix/nix
    cp src/resources/*.nix $out/share/trix/nix/
    cp direnvrc $out/share/trix/
  '';

  doInstallCheck = true;
  nativeInstallCheckInputs = [ versionCheckHook ];
  versionCheckProgramArg = "--version";

  meta = {
    description = "trick yourself into flakes";
    homepage = "https://github.com/aanderse/trix";
    license = lib.licenses.mit;
    mainProgram = "trix";
    maintainers = with lib.maintainers; [
      aanderse
      drupol
    ];
    platforms = lib.platforms.all;
  };
}
