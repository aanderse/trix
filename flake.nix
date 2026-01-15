{
  description = "trix - A Nix flake CLI that never copies your project to the store";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  };

  outputs = { self, nixpkgs }:
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
              # Use system libgit2 (same version as nix) to avoid conflicts
              pkgs.libgit2
              # For bindgen (generates FFI bindings)
              pkgs.llvmPackages.libclang
            ];

            # bindgen needs to find libclang
            LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";

            # bindgen needs libc headers
            BINDGEN_EXTRA_CLANG_ARGS = "-isystem ${pkgs.glibc.dev}/include";

            # Use system libgit2 instead of bundled version to avoid conflicts with nix's libgit2
            LIBGIT2_NO_VENDOR = "1";

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
        in
        {
          default = pkgs.rustPlatform.buildRustPackage {
            pname = "trix";
            version = "0.1.0";
            src = ./.;
            cargoLock = {
              lockFile = ./Cargo.lock;
            };

            nativeBuildInputs = [
              pkgs.pkg-config
              pkgs.llvmPackages.libclang
            ];

            buildInputs = [
              pkgs.nix
              # Use system libgit2 (same version as nix) to avoid conflicts
              pkgs.libgit2
            ];

            # bindgen needs to find libclang
            LIBCLANG_PATH = "${pkgs.llvmPackages.libclang.lib}/lib";

            # bindgen needs libc headers
            BINDGEN_EXTRA_CLANG_ARGS = "-isystem ${pkgs.glibc.dev}/include";

            # Use system libgit2 instead of bundled version to avoid conflicts with nix's libgit2
            LIBGIT2_NO_VENDOR = "1";

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
