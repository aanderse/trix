{
  lib,
  python3Packages,
}:

let
  fs = lib.fileset;
  sourceFiles = fs.difference
    (fs.unions [
      ./pyproject.toml
      ./src
      ./nix
      ./direnvrc
    ])
    # Exclude .pyc files (contents of __pycache__ directories)
    (fs.fileFilter (file: file.hasExt "pyc") ./src);
in

python3Packages.buildPythonApplication {
  pname = "trix";
  version = "0.1.0";
  pyproject = true;

  src = fs.toSource {
    root = ./.;
    fileset = sourceFiles;
  };

  build-system = [ python3Packages.hatchling ];

  dependencies = [ python3Packages.click ];

  nativeCheckInputs = [ python3Packages.pytest ];

  # Tests require nix commands
  doCheck = false;

  postInstall = ''
    mkdir -p $out/share/trix
    cp -r nix $out/share/trix/
    cp direnvrc $out/share/trix/

    # Shell completions
    mkdir -p $out/share/bash-completion/completions
    mkdir -p $out/share/zsh/site-functions
    mkdir -p $out/share/fish/vendor_completions.d
    $out/bin/trix completion bash > $out/share/bash-completion/completions/trix
    $out/bin/trix completion zsh > $out/share/zsh/site-functions/_trix
    $out/bin/trix completion fish > $out/share/fish/vendor_completions.d/trix.fish
  '';

  meta = {
    description = "trick yourself into flakes";
    homepage = "https://github.com/aanderse/trix";
    license = lib.licenses.mit;
    mainProgram = "trix";
  };
}
