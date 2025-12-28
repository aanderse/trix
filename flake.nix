{
  description = "trick yourself into flakes";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";
  };

  outputs =
    inputs:
    let
      forAllSystems =
        fn:
        inputs.nixpkgs.lib.genAttrs inputs.nixpkgs.lib.systems.flakeExposed (
          system:
          fn (
            import inputs.nixpkgs {
              inherit system;
              overlays = [
                inputs.self.overlays.default
              ];
            }
          )
        );
    in
    {
      packages = forAllSystems (
        pkgs:
        {
          inherit (pkgs) trix;
          default = pkgs.trix;
        }
      );

      formatter = forAllSystems (pkgs: pkgs.nixfmt-tree);

      devShells = forAllSystems (pkgs: {
        default = pkgs.mkShell {
          packages = with pkgs; [
            cargo
            clippy
            rust-analyzer
            rustc
            rustfmt
          ];

          shellHook = ''
            export PATH=$PWD/target/debug:$PATH
            export RUST_SRC_PATH="${pkgs.rust.packages.stable.rustPlatform.rustLibSrc}";
          '';
        };
      });

      overlays.default = final: prev: {
        trix = final.callPackage ./package.nix { };
      };
    };
}
