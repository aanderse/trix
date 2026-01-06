{
  description = "trix - A Nix flake CLI that never copies your project to the store";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    nix-bindings-rust = {
      url = "github:aanderse/nix-bindings-rust/8f6ec2ec5c3ba8ab33126ec79de7702835592902";
      flake = false;  # It's not a flake, just source
    };
  };

  outputs = { self, nixpkgs, nix-bindings-rust }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" "x86_64-darwin" "aarch64-darwin" ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f system);
    in
    {
      devShells = forAllSystems (system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
        in
        {
          default = pkgs.mkShell {
            buildInputs = [
              pkgs.cargo
              pkgs.rustc
              pkgs.rust-analyzer
              pkgs.cargo-watch
              pkgs.pkg-config
              # Nix libraries for nix-bindings
              pkgs.nix
              # For bindgen (generates FFI bindings)
              pkgs.llvmPackages.libclang
            ];

            # bindgen needs to find libclang
            LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";

            # bindgen needs libc headers
            BINDGEN_EXTRA_CLANG_ARGS = "-isystem ${pkgs.glibc.dev}/include";

            shellHook = ''
              echo "trix development shell"
              echo "Run 'cargo build' to build"
              echo "Run 'cargo test' to test"
            '';
          };
        });

      packages = forAllSystems (system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
          # Create combined source tree so ../nix-bindings-rust resolves
          combinedSrc = pkgs.runCommand "trix-combined-src" {} ''
            mkdir -p $out/trix $out/nix-bindings-rust
            cp -r ${./.}/* $out/trix/
            cp -r ${nix-bindings-rust}/* $out/nix-bindings-rust/
          '';
        in
        {
          default = pkgs.rustPlatform.buildRustPackage {
            pname = "trix";
            version = "0.1.0";
            src = combinedSrc;
            sourceRoot = "trix-combined-src/trix";
            cargoLock = {
              lockFile = ./Cargo.lock;
            };

            nativeBuildInputs = [
              pkgs.pkg-config
              pkgs.llvmPackages.libclang
            ];

            buildInputs = [
              pkgs.nix
            ];

            # bindgen needs to find libclang
            LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";

            # bindgen needs libc headers
            BINDGEN_EXTRA_CLANG_ARGS = "-isystem ${pkgs.glibc.dev}/include";

            # Tests require Nix store access which isn't available in sandbox
            doCheck = false;

            # Install additional files
            postInstall = ''
              mkdir -p $out/share/trix
              cp ${./direnvrc} $out/share/trix/direnvrc
            '';
          };
        });
    };
}
