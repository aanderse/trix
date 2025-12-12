{
  description = "trick yourself into flakes";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs?ref=nixos-unstable";
  };

  outputs =
    {
      self,
      nixpkgs,
      ...
    }:
    let
      supportedSystems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];
      forAllSystems = nixpkgs.lib.genAttrs supportedSystems;
    in
    {
      packages = forAllSystems (
        system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
        in
        {
          trix = pkgs.callPackage ./package.nix { };
          default = self.packages.${system}.trix;
        }
      );

      devShells = forAllSystems (
        system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
          python = pkgs.python3.withPackages (p: [
            p.click
            p.pytest
          ]);
          # Create a wrapper script that calls trix via python -m
          trixWrapper = pkgs.writeShellScriptBin "trix" ''
            exec ${python}/bin/python -m trix.cli "$@"
          '';
        in
        {
          default = pkgs.mkShell {
            packages = [
              python
              trixWrapper
              pkgs.ruff
            ];

            shellHook = ''
              export PYTHONPATH=$PWD/src

              echo 'trix --help'
              trix --help
            '';
          };
        }
      );
    };
}
