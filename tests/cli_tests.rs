use predicates::prelude::*;
use std::fs;
use tempfile::tempdir;

#[test]
fn test_hash_help() {
    let mut cmd = assert_cmd::cargo::cargo_bin_cmd!("trix");
    cmd.args(["hash", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Compute and convert cryptographic hashes",
        ));
}

#[test]
fn test_cli_help() {
    let mut cmd = assert_cmd::cargo::cargo_bin_cmd!("trix");
    cmd.arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("trix"));
}

#[test]
fn test_flake_init_help() {
    let mut cmd = assert_cmd::cargo::cargo_bin_cmd!("trix");
    cmd.args(["flake", "init", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Create a flake in the current directory",
        ));
}

#[test]
fn test_eval_basic() {
    let dir = tempdir().unwrap();
    let flake_nix = dir.path().join("flake.nix");
    fs::write(
        &flake_nix,
        r#"{
  outputs = { self }: {
    default = "hello";
    testValue = 42;
  };
}"#,
    )
    .unwrap();

    let mut cmd = assert_cmd::cargo::cargo_bin_cmd!("trix");
    let assert = cmd
        .args(["eval", ".#testValue"])
        .current_dir(dir.path())
        .assert();

    // Handle case where nix is missing (CI/local dev environment variations)
    let output = assert.get_output();
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() && stderr.contains("not found") {
        eprintln!("Skipping test_eval_basic: nix command not found");
        return;
    }

    assert.success().stdout(predicate::str::contains("42"));
}

#[test]
fn test_lock_basic() {
    let dir = tempdir().unwrap();
    let flake_nix = dir.path().join("flake.nix");
    fs::write(
        &flake_nix,
        r#"{
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  outputs = { self, nixpkgs }: { };
}"#,
    )
    .unwrap();

    let mut cmd = assert_cmd::cargo::cargo_bin_cmd!("trix");
    let assert = cmd.args(["flake", "lock"]).current_dir(dir.path()).assert();

    let output = assert.get_output();
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() && stderr.contains("not found") {
        eprintln!("Skipping test_lock_basic: nix command not found");
        return;
    }

    assert.success();
    assert!(dir.path().join("flake.lock").exists());
}

#[test]
fn test_fmt_help() {
    let mut cmd = assert_cmd::cargo::cargo_bin_cmd!("trix");
    cmd.args(["fmt", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Format files using the flake's formatter",
        ));
}

#[test]
fn test_fmt_basic() {
    let dir = tempdir().unwrap();
    let flake_nix = dir.path().join("flake.nix");

    // Create a mock formatter that appends "formatted" to the file
    fs::write(
        &flake_nix,
        r##"{
  outputs = { self }: {
    formatter.${builtins.currentSystem} = derivation {
      name = "mock-fmt";
      system = builtins.currentSystem;
      builder = "/bin/sh";
      args = [ "-c" ''
        mkdir -p $out/bin
        echo "#!/bin/sh" > $out/bin/mock-fmt
        echo "for f in \$@; do echo 'formatted' >> \$f; done" >> $out/bin/mock-fmt
        chmod +x $out/bin/mock-fmt
      '' ];
    };
  };
}"##,
    )
    .unwrap();

    let file_to_format = dir.path().join("test.txt");
    fs::write(&file_to_format, "original content\n").unwrap();

    let mut cmd = assert_cmd::cargo::cargo_bin_cmd!("trix");
    let assert = cmd
        .args(["fmt", ".", "--", "test.txt"])
        .current_dir(dir.path())
        .assert();

    let output = assert.get_output();
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() && (stderr.contains("not found") || stderr.contains("No such file"))
    {
        eprintln!("Skipping test_fmt_basic: nix command not found or broken");
        return;
    }

    assert.success();

    let content = fs::read_to_string(&file_to_format).unwrap();
    assert!(content.contains("original content"));
    assert!(content.contains("formatted"));
}
