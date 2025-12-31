use anyhow::Result;
use std::fs;

use tempfile::tempdir;

#[test]
fn test_fmt_without_flake() -> Result<()> {
    let dir = tempdir()?;
    let dir_path = dir.path();

    // Create a dummy file to format
    let file_to_format = dir_path.join("file.txt");
    fs::write(&file_to_format, "content")?;

    // Create default.nix with a formatter
    let default_nix = r#"
        { system ? builtins.currentSystem }:
        let
          pkgs = import <nixpkgs> { inherit system; };
        in
        {
          formatter.${system} = pkgs.writeShellScriptBin "fmt-dummy" ''
            for file in "$@"; do
              echo "formatted" >> "$file"
            done
          '';
        }
    "#;
    fs::write(dir_path.join("default.nix"), default_nix)?;

    // Run trix fmt
    let mut cmd = assert_cmd::cargo::cargo_bin_cmd!("trix");
    cmd.current_dir(dir_path)
        .arg("fmt")
        .arg(".")
        .arg("--")
        .arg("file.txt");

    cmd.assert().success();

    // Verify the file was changed
    let content = fs::read_to_string(file_to_format)?;
    assert!(content.contains("formatted"));

    Ok(())
}

#[test]
fn test_fmt_with_flake() -> Result<()> {
    let dir = tempdir()?;
    let dir_path = dir.path();

    // Create a dummy file to format
    let file_to_format = dir_path.join("file.txt");
    fs::write(&file_to_format, "content")?;

    // Create flake.nix with a formatter
    let flake_nix = r#"
        {
          inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
          outputs = { self, nixpkgs }: {
            formatter.x86_64-linux = nixpkgs.legacyPackages.x86_64-linux.writeShellScriptBin "fmt-dummy-flake" ''
              for file in "$@"; do
                echo "formatted-by-flake" >> "$file"
              done
            '';
          };
        }
    "#;
    fs::write(dir_path.join("flake.nix"), flake_nix)?;

    // Create empty flake.lock to avoid network overhead/locking if possible (though nix will likely try to fetch inputs)
    // To make this robust and fast without net access, we might need a mocked inputs or registry override.
    // However, existing tests imply some nix functionality.
    // For a reliable test without fetching nixpkgs, we should use a dummy input or just system nixpkgs if allowed.
    // But since we are testing 'trix', let's stick to a simpler self-contained flake that doesn't strictly depend on external inputs if possible.
    // Actually, simple flake outputs don't strictly need inputs if we construct derivations manually or use builtins.

    // BETTER FLAKE: No inputs, using builtins.toFile or similar to avoid fetching nixpkgs
    // Use a minimal flake that doesn't require fetching
    let minimal_flake = r#"
        {
          outputs = { self }: {
            formatter.x86_64-linux = (import <nixpkgs> {}).writeShellScriptBin "fmt-dummy-flake" ''
              for file in "$@"; do
                echo "formatted-by-flake" >> "$file"
              done
            '';
          };
        }
    "#;
    fs::write(dir_path.join("flake.nix"), minimal_flake)?;

    // Run trix fmt
    let mut cmd = assert_cmd::cargo::cargo_bin_cmd!("trix");
    cmd.current_dir(dir_path)
        .arg("fmt")
        .arg(".")
        .arg("--")
        .arg("file.txt");

    cmd.assert().success();

    // Verify the file was changed
    let content = fs::read_to_string(file_to_format)?;
    assert!(content.contains("formatted-by-flake"));

    Ok(())
}
