//! Nix command wrappers.
//!
//! This module provides functions to run nix commands (nix-build, nix-shell, nix-instantiate)
//! with the trix evaluation wrapper.

use anyhow::{Context, Result};
use once_cell::sync::Lazy;
use rayon::prelude::*;
use std::collections::HashMap;
use std::env;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Empty lock expression for flakes without a lock file
pub const EMPTY_LOCK_EXPR: &str =
    r#"{ nodes = { root = { inputs = {}; }; }; root = "root"; version = 7; }"#;

/// Cached nix dir path
static NIX_DIR_CACHE: Lazy<Mutex<Option<PathBuf>>> = Lazy::new(|| Mutex::new(None));

/// Cached system value
static SYSTEM_CACHE: Lazy<Mutex<Option<String>>> = Lazy::new(|| Mutex::new(None));

/// Cached store dir value
static STORE_DIR_CACHE: Lazy<Mutex<Option<String>>> = Lazy::new(|| Mutex::new(None));

/// Get environment suitable for spawning nix commands.
///
/// Removes TMPDIR to let nix/bash use the system default (/tmp).
/// This avoids issues where TMPDIR points to a directory created by
/// a parent nix-shell that may be cleaned up unexpectedly.
pub fn get_clean_env() -> HashMap<String, String> {
    let mut env_map: HashMap<String, String> = env::vars().collect();
    env_map.remove("TMPDIR");
    env_map
}

/// Print a warning message to stderr in nix style.
pub fn warn(msg: &str) {
    tracing::warn!("{}", msg);
}

/// Get the path to Nix support files.
///
/// Walks up from the executable to find nix files in:
/// - Development: src/resources/ (from target/debug/trix or target/release/trix)
/// - Installed: share/trix/nix/ (from bin/trix)
pub fn get_nix_dir() -> Result<PathBuf> {
    // Check cache first
    {
        let cache = NIX_DIR_CACHE.lock().unwrap();
        if let Some(ref dir) = *cache {
            return Ok(dir.clone());
        }
    }

    let nix_dir = find_nix_dir()?;

    // Cache the result
    {
        let mut cache = NIX_DIR_CACHE.lock().unwrap();
        *cache = Some(nix_dir.clone());
    }

    Ok(nix_dir)
}

fn find_nix_dir() -> Result<PathBuf> {
    let exe = env::current_exe().context("Cannot determine executable path")?;

    // Walk up from executable looking for nix files
    for parent in exe.ancestors().skip(1) {
        // Installed: $out/share/trix/nix/
        let installed = parent.join("share/trix/nix");
        if installed.join("eval.nix").exists() {
            return Ok(installed);
        }

        // Development: repo/src/resources/
        let dev = parent.join("src/resources");
        if dev.join("eval.nix").exists() {
            return Ok(dev);
        }
    }

    anyhow::bail!("Cannot find nix/ directory")
}

/// Get the Nix expression to load the flake lock file.
///
/// Returns either an expression to read the existing lock file,
/// or an empty lock structure if no lock file exists.
pub fn get_lock_expr(flake_dir: &Path) -> String {
    let lock_file = flake_dir.join("flake.lock");
    if lock_file.exists() {
        format!(
            "builtins.fromJSON (builtins.readFile {}/flake.lock)",
            flake_dir.display()
        )
    } else {
        EMPTY_LOCK_EXPR.to_string()
    }
}

/// Get the Nix expression for the 'self' input metadata.
///
/// Matches Nix's behavior:
/// - Clean repo: rev, shortRev, lastModified, lastModifiedDate
/// - Dirty repo: dirtyRev, dirtyShortRev, lastModified, lastModifiedDate
/// - Always: submodules
///
/// Note: revCount is intentionally omitted (see git.rs for explanation).
pub fn get_self_info_expr(flake_dir: &Path) -> String {
    let git_info = crate::git::get_git_info(flake_dir).unwrap_or_default();

    // Construct selfInfo attrset
    let mut parts = Vec::new();

    // Clean repo attributes
    if let Some(rev) = git_info.rev {
        parts.push(format!("rev = \"{}\";", rev));
    }
    if let Some(short_rev) = git_info.short_rev {
        parts.push(format!("shortRev = \"{}\";", short_rev));
    }

    // Dirty repo attributes
    if let Some(dirty_rev) = git_info.dirty_rev {
        parts.push(format!("dirtyRev = \"{}\";", dirty_rev));
    }
    if let Some(dirty_short_rev) = git_info.dirty_short_rev {
        parts.push(format!("dirtyShortRev = \"{}\";", dirty_short_rev));
    }

    // Always included attributes
    if let Some(last_modified) = git_info.last_modified {
        parts.push(format!("lastModified = {};", last_modified));
    }
    if let Some(date) = git_info.last_modified_date {
        parts.push(format!("lastModifiedDate = \"{}\";", date));
    }
    parts.push(format!(
        "submodules = {};",
        if git_info.submodules { "true" } else { "false" }
    ));

    format!("{{ {} }}", parts.join(" "))
}

/// Convert a dotted attribute path to a Nix list expression.
///
/// Examples:
///     "packages.x86_64-linux.hello" -> '["packages" "x86_64-linux" "hello"]'
///     "" -> "[]"
pub fn attr_to_nix_list(attr: &str) -> String {
    let parts: Vec<&str> = attr.split('.').filter(|p| !p.is_empty()).collect();
    if parts.is_empty() {
        return "[]".to_string();
    }
    let quoted: Vec<String> = parts.iter().map(|p| format!("\"{}\"", p)).collect();
    format!("[{}]", quoted.join(" "))
}

/// Generate the common Nix let-bindings for evaluation.
///
/// Returns Nix code that sets up the environment (helpers, outputs, etc.) for
/// either a flake (via flake.nix) or a legacy project (via default.nix).
pub fn get_eval_preamble(flake_dir: &Path) -> Result<String> {
    let nix_dir = get_nix_dir()?;
    let is_flake = flake_dir.join("flake.nix").exists();

    let (lock_expr, self_info_expr) = if is_flake {
        (get_lock_expr(flake_dir), get_self_info_expr(flake_dir))
    } else {
        // Legacy mode: no flake.nix means no lock or self input metadata.
        ("{}".to_string(), "{}".to_string())
    };

    Ok(format!(
        r#"
      context = import {nix_dir}/get_eval_preamble.nix {{
        flakeDir = {flake_dir};
        isFlake = {is_flake};
        lock = {lock_expr};
        selfInfo = {self_info_expr};
        nixDir = {nix_dir};
      }};
      inherit (context) helpers hasPath getPath resolveAttrPath outputs;
    "#,
        nix_dir = nix_dir.display(),
        flake_dir = flake_dir.display(),
        is_flake = is_flake,
        lock_expr = lock_expr,
        self_info_expr = self_info_expr,
    ))
}

/// Get the current Nix system (e.g., x86_64-linux). Result is cached.
pub fn get_system() -> Result<String> {
    // Check cache first
    {
        let cache = SYSTEM_CACHE.lock().unwrap();
        if let Some(ref system) = *cache {
            return Ok(system.clone());
        }
    }

    let mut cmd = crate::command::NixCommand::new("nix-instantiate");
    cmd.args(["--eval", "--json", "--expr", "builtins.currentSystem"]);

    let system = cmd.json().unwrap_or_else(|_| get_fallback_system());

    // Cache the result
    {
        let mut cache = SYSTEM_CACHE.lock().unwrap();
        *cache = Some(system.clone());
    }

    Ok(system)
}

fn get_fallback_system() -> String {
    let arch = std::env::consts::ARCH;
    let os = std::env::consts::OS;
    // Map Rust arch/os to Nix conventions
    let nix_arch = match arch {
        "x86_64" => "x86_64",
        "aarch64" => "aarch64",
        "x86" => "i686",
        _ => arch,
    };
    format!("{}-{}", nix_arch, os)
}

/// Get the Nix store directory (e.g., /nix/store). Result is cached.
pub fn get_store_dir() -> Result<String> {
    // Check cache first
    {
        let cache = STORE_DIR_CACHE.lock().unwrap();
        if let Some(ref store_dir) = *cache {
            return Ok(store_dir.clone());
        }
    }

    let mut cmd = crate::command::NixCommand::new("nix-instantiate");
    cmd.args(["--eval", "--json", "--expr", "builtins.storeDir"]);

    let store_dir = cmd.json().unwrap_or_else(|_| "/nix/store".to_string());

    // Cache the result
    {
        let mut cache = STORE_DIR_CACHE.lock().unwrap();
        *cache = Some(store_dir.clone());
    }

    Ok(store_dir)
}

/// Options for nix-build
#[derive(Debug, Default)]
pub struct BuildOptions {
    pub out_link: Option<String>,
    pub extra_args: Vec<(String, String)>,
    pub extra_argstrs: Vec<(String, String)>,
    pub store: Option<String>,
}

/// Run nix-build with eval.nix wrapper.
///
/// Returns store path if capture_output=true, else None.
pub fn run_nix_build(
    flake_dir: &Path,
    attr: &str,
    options: &BuildOptions,
    capture_output: bool,
) -> Result<Option<String>> {
    let mut cmd = crate::command::NixCommand::new("nix-build");

    if flake_dir.join("flake.nix").exists() {
        let nix_dir = get_nix_dir()?;
        let self_info_expr = get_self_info_expr(flake_dir);

        cmd.arg(nix_dir.join("eval.nix"));
        cmd.args(["--arg", "flakeDir", &flake_dir.display().to_string()]);
        cmd.args(["--arg", "selfInfo", &self_info_expr]);
        cmd.args(["--argstr", "attr", attr]);
    } else {
        // Legacy mode: use standard nix-build with attribute path.
        cmd.arg(flake_dir);
        cmd.args(["-A", attr]);
    }

    if let Some(ref store) = options.store {
        cmd.args(["--store", store]);
    }

    for (name, expr) in &options.extra_args {
        cmd.args(["--arg", name, expr]);
    }

    for (name, value) in &options.extra_argstrs {
        cmd.args(["--argstr", name, value]);
    }

    match &options.out_link {
        Some(link) => {
            cmd.args(["-o", link]);
        }
        None => {
            cmd.arg("--no-link");
        }
    }

    if capture_output {
        Ok(Some(cmd.output()?))
    } else {
        cmd.run()?;
        Ok(None)
    }
}

/// Options for nix-shell
#[derive(Debug, Default)]
pub struct ShellOptions {
    pub command: Option<String>,
    pub extra_args: Vec<(String, String)>,
    pub extra_argstrs: Vec<(String, String)>,
    pub store: Option<String>,
    pub bash_prompt: Option<String>,
    pub bash_prompt_prefix: Option<String>,
    pub bash_prompt_suffix: Option<String>,
}

/// Run nix-shell with eval.nix wrapper. Replaces current process.
pub fn run_nix_shell(flake_dir: &Path, attr: &str, options: &ShellOptions) -> Result<()> {
    let nix_dir = get_nix_dir()?;
    let self_info_expr = get_self_info_expr(flake_dir);

    let mut cmd = crate::command::NixCommand::new("nix-shell");
    cmd.arg(nix_dir.join("eval.nix"));
    cmd.args(["--arg", "flakeDir", &flake_dir.display().to_string()]);
    cmd.args(["--arg", "selfInfo", &self_info_expr]);
    cmd.args(["--argstr", "attr", attr]);

    if let Some(ref store) = options.store {
        cmd.args(["--store", store]);
    }

    for (name, expr) in &options.extra_args {
        cmd.args(["--arg", name, expr]);
    }

    for (name, value) in &options.extra_argstrs {
        cmd.args(["--argstr", name, value]);
    }

    if let Some(ref command) = options.command {
        cmd.args(["--command", command]);
    }

    // Set up environment for bash prompt and shell
    let mut env_overrides = HashMap::new();

    // Set NIX_BUILD_SHELL to bash if not already set, to avoid nix-shell trying
    // to get bashInteractive from <nixpkgs> (which fails without NIX_PATH set).
    if env::var("NIX_BUILD_SHELL").is_err() {
        env_overrides.insert("NIX_BUILD_SHELL".to_string(), "bash".to_string());
    }

    if let Some(ref prompt) = options.bash_prompt {
        let escaped = prompt.replace('\'', "'\\''");
        env_overrides.insert(
            "PROMPT_COMMAND".to_string(),
            format!("PS1='{}'; unset PROMPT_COMMAND", escaped),
        );
    } else if options.bash_prompt_prefix.is_some() || options.bash_prompt_suffix.is_some() {
        let prefix = options.bash_prompt_prefix.as_deref().unwrap_or("");
        let suffix = options.bash_prompt_suffix.as_deref().unwrap_or("");
        let default_prompt = r"\[\e[0;1;35m\][nix-shell:\w]$\[\e[0m\] ";
        let full_prompt = format!("{}{}{}", prefix, default_prompt, suffix);
        let escaped = full_prompt.replace('\'', "'\\''");
        env_overrides.insert(
            "PROMPT_COMMAND".to_string(),
            format!("PS1='{}'; unset PROMPT_COMMAND", escaped),
        );
    }

    if !env_overrides.is_empty() {
        cmd.envs(env_overrides);
    }

    cmd.exec()
}

/// Options for nix eval
#[derive(Debug, Default)]
pub struct EvalOptions {
    pub output_json: bool,
    pub raw: bool,
    pub apply_fn: Option<String>,
    pub extra_args: Vec<(String, String)>,
    pub extra_argstrs: Vec<(String, String)>,
    pub expr: Option<String>,
    pub store: Option<String>,
    pub quiet: bool,
}

/// Evaluate a flake attribute or raw expression and return the result.
pub fn run_nix_eval(flake_dir: Option<&Path>, attr: &str, options: &EvalOptions) -> Result<String> {
    let nix_expr = if let Some(ref expr) = options.expr {
        // Raw expression evaluation
        if let Some(ref apply_fn) = options.apply_fn {
            format!("({}) ({})", apply_fn, expr)
        } else {
            expr.clone()
        }
    } else {
        // Flake-based evaluation
        let flake_dir = flake_dir.context("flake_dir required for flake evaluation")?;
        let preamble = get_eval_preamble(flake_dir)?;

        // Handle empty attr (from .#) -> "default"
        let effective_attr = if attr.is_empty() { "default" } else { attr };

        let apply_part = if let Some(ref apply_fn) = options.apply_fn {
            format!("({}) value", apply_fn)
        } else {
            "value".to_string()
        };

        format!(
            r#"
        let
          {preamble}
          value = resolveAttrPath "{attr}" outputs;
        in {apply_part}
        "#,
            preamble = preamble,
            attr = effective_attr,
            apply_part = apply_part,
        )
    };

    let mut cmd = crate::command::NixCommand::new("nix-instantiate");
    cmd.args([
        "--eval",
        "--strict",
        "--read-write-mode",
        "--expr",
        &nix_expr,
    ]);

    if let Some(ref store) = options.store {
        cmd.args(["--store", store]);
    }

    if options.output_json {
        cmd.arg("--json");
    }

    for (name, expr) in &options.extra_args {
        cmd.args(["--arg", name, expr]);
    }

    for (name, value) in &options.extra_argstrs {
        cmd.args(["--argstr", name, value]);
    }

    match cmd.output() {
        Ok(stdout) => {
            let mut result = stdout;
            // Handle --raw: strip quotes from string output
            if options.raw && result.starts_with('"') && result.ends_with('"') {
                result = result[1..result.len() - 1].to_string();
                result = unescape_nix_string(&result);
            }
            Ok(result)
        }
        Err(e) => {
            if !options.quiet {
                tracing::error!("{}", e);
            }
            Err(e)
        }
    }
}

/// Unescape a Nix string literal (handles standard escape sequences).
fn unescape_nix_string(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => result.push('\n'),
                Some('r') => result.push('\r'),
                Some('t') => result.push('\t'),
                Some('\\') => result.push('\\'),
                Some('"') => result.push('"'),
                Some('$') => result.push('$'),
                Some(other) => {
                    // Unknown escape, preserve as-is
                    result.push('\\');
                    result.push(other);
                }
                None => result.push('\\'),
            }
        } else {
            result.push(c);
        }
    }

    result
}

/// Check if a flake has a specific attribute path.
pub fn flake_has_attr(flake_dir: &Path, attr: &str) -> Result<bool> {
    let preamble = get_eval_preamble(flake_dir)?;
    let attr_list = attr_to_nix_list(attr);

    let nix_expr = format!(
        r#"
    let
      {preamble}
      attrPath = {attr_list};
    in hasPath attrPath outputs
    "#,
        preamble = preamble,
        attr_list = attr_list,
    );

    let mut cmd = crate::command::NixCommand::new("nix-instantiate");
    cmd.args(["--eval", "--read-write-mode", "--expr", &nix_expr]);

    match cmd.output() {
        Ok(stdout) => Ok(stdout.trim() == "true"),
        Err(_) => Ok(false),
    }
}

/// Get the main program name for a package.
///
/// Determines the executable name by inspecting the package's metadata
/// (meta.mainProgram, pname, or name).
pub fn get_package_main_program(flake_dir: &Path, attr: &str) -> Result<String> {
    let nix_dir = get_nix_dir()?;
    let preamble = get_eval_preamble(flake_dir)?;

    // Evaluate the package to get mainProgram, pname, or name
    // Uses resolveAttrPath from helpers.nix for packages -> legacyPackages fallback
    let nix_expr = format!(
        r#"
    let
      {preamble}
    in import {nix_dir}/get_package_main_program.nix {{
      inherit outputs resolveAttrPath;
      attr = "{attr}";
    }}
    "#,
        preamble = preamble,
        nix_dir = nix_dir.display(),
        attr = attr,
    );

    let mut cmd = crate::command::NixCommand::new("nix-instantiate");
    cmd.args(["--eval", "--json", "--read-write-mode", "--expr", &nix_expr]);

    let output = cmd.output()?;
    let program: Option<String> = serde_json::from_str(&output)?;
    program.context("Could not determine main program for package")
}

/// Run nix repl with flake context loaded. Replaces current process.
pub fn run_nix_repl(flake_dir: &Path) -> Result<()> {
    let nix_dir = get_nix_dir()?;
    let self_info_expr = get_self_info_expr(flake_dir);

    let mut cmd = crate::command::NixCommand::new("nix");
    cmd.args(["repl", "--file"]);
    cmd.arg(nix_dir.join("repl.nix"));
    cmd.args(["--arg", "flakeDir", &flake_dir.display().to_string()]);
    cmd.args(["--arg", "selfInfo", &self_info_expr]);

    cmd.exec()
}

/// Get the derivation path for a flake attribute without building.
pub fn get_derivation_path(flake_dir: &Path, attr: &str) -> Result<String> {
    let nix_dir = get_nix_dir()?;
    let self_info_expr = get_self_info_expr(flake_dir);

    let mut cmd = crate::command::NixCommand::new("nix-instantiate");
    cmd.arg(nix_dir.join("eval.nix"));
    cmd.args(["--arg", "flakeDir", &flake_dir.display().to_string()]);
    cmd.args(["--arg", "selfInfo", &self_info_expr]);
    cmd.args(["--argstr", "attr", attr]);

    cmd.output()
}

/// Get the output store path from a derivation path.
pub fn get_store_path_from_drv(drv_path: &str) -> Result<String> {
    let mut cmd = crate::command::NixCommand::new("nix-store");
    cmd.args(["-q", "--outputs", drv_path]);

    let stdout = cmd.output()?;
    Ok(stdout.lines().next().unwrap_or("").to_string())
}

/// Get the build log for a store path.
pub fn get_build_log(store_path: &str) -> Option<String> {
    let mut cmd = crate::command::NixCommand::new("nix-store");
    cmd.args(["--read-log", store_path]);
    cmd.output().ok()
}

/// Get the structure of flake outputs.
pub fn eval_flake_outputs(
    flake_dir: &Path,
    all_systems: bool,
    show_legacy: bool,
) -> Result<Option<serde_json::Value>> {
    let categories = match get_flake_output_categories(flake_dir)? {
        Some(c) => c,
        None => return Ok(None),
    };

    if categories.is_empty() {
        return Ok(Some(serde_json::json!({})));
    }

    tracing::debug!("+ Evaluating {} categories in parallel", categories.len());

    let results: Vec<(String, Option<serde_json::Value>)> = categories
        .into_par_iter()
        .map(|cat| {
            let res = eval_flake_output_category(flake_dir, &cat, all_systems, show_legacy);
            match res {
                Ok(val) => (cat, val),
                Err(e) => {
                    tracing::debug!("Error evaluating category {}: {}", cat, e);
                    // Return unknown marker instead of None so the category still shows
                    (cat, Some(serde_json::json!({ "_unknown": true })))
                }
            }
        })
        .collect();

    let mut map = serde_json::Map::new();
    for (cat, val) in results {
        if let Some(v) = val {
            map.insert(cat, v);
        }
    }

    Ok(Some(serde_json::Value::Object(map)))
}

/// Evaluate a single flake output category.
pub fn eval_flake_output_category(
    flake_dir: &Path,
    category: &str,
    all_systems: bool,
    show_legacy: bool,
) -> Result<Option<serde_json::Value>> {
    let preamble = get_eval_preamble(flake_dir)?;
    let all_systems_nix = if all_systems { "true" } else { "false" };
    let show_legacy_nix = if show_legacy { "true" } else { "false" };

    let nix_dir = get_nix_dir()?;
    let expr = format!(
        r#"
    let
      {preamble}
      allSystemsFlag = {all_systems_nix};
      showLegacyFlag = {show_legacy_nix};
    in import {nix_dir}/eval_category.nix {{
      inherit outputs allSystemsFlag showLegacyFlag;
      category = "{category}";
    }}
    "#,
        preamble = preamble,
        all_systems_nix = all_systems_nix,
        show_legacy_nix = show_legacy_nix,
        nix_dir = nix_dir.display(),
        category = category
    );

    let mut cmd = crate::command::NixCommand::new("nix-instantiate");
    cmd.args([
        "--eval",
        "--json",
        "--strict",
        "--read-write-mode",
        "--expr",
        &expr,
    ]);

    match cmd.json() {
        Ok(result) => Ok(Some(result)),
        Err(e) => Err(e),
    }
}

/// Get the list of top-level output category names.
pub fn get_flake_output_categories(flake_dir: &Path) -> Result<Option<Vec<String>>> {
    let preamble = get_eval_preamble(flake_dir)?;

    let expr = format!(
        r#"
    let
      {preamble}
    in builtins.attrNames outputs
    "#,
        preamble = preamble,
    );

    let mut cmd = crate::command::NixCommand::new("nix-instantiate");
    cmd.args(["--eval", "--json", "--read-write-mode", "--expr", &expr]);

    tracing::debug!("+ nix-instantiate --eval ... (getting output categories)");

    match cmd.json() {
        Ok(result) => Ok(Some(result)),
        Err(e) => {
            tracing::debug!("{}", e);
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_get_clean_env() {
        let env = get_clean_env();
        // Check that PATH is preserved (it should be in any reasonable environment)
        assert!(env.contains_key("PATH"));
        // TMPDIR should be removed
        assert!(!env.contains_key("TMPDIR"));
    }

    #[test]
    fn test_get_nix_dir() {
        let nix_dir = get_nix_dir().expect("Failed to get nix dir");
        assert!(nix_dir.is_dir());
        assert!(nix_dir.join("eval.nix").exists());
        assert!(nix_dir.join("inputs.nix").exists());
    }

    #[test]
    fn test_get_system() {
        // Clear cache first (or just trust that it works if already called)
        let system = get_system().expect("Failed to get system");
        assert!(!system.is_empty());
        assert!(system.contains('-'));
    }

    #[test]
    fn test_get_fallback_system() {
        let sys = get_fallback_system();
        assert!(sys.contains('-'));
    }

    #[test]
    fn test_attr_to_nix_list() {
        assert_eq!(attr_to_nix_list(""), "[]");
        assert_eq!(attr_to_nix_list("a"), "[\"a\"]");
        assert_eq!(
            attr_to_nix_list("packages.x86_64-linux.hello"),
            "[\"packages\" \"x86_64-linux\" \"hello\"]"
        );
    }

    #[test]
    fn test_options_defaults() {
        let opts = BuildOptions::default();
        assert!(opts.out_link.is_none());
        assert!(opts.out_link.is_none());

        let opts = ShellOptions::default();
        assert!(opts.command.is_none());
    }

    #[test]
    fn test_eval_expr_simple() {
        let result = eval_expr("1 + 1").expect("Failed to eval expr");
        assert_eq!(result, serde_json::json!(2));

        let result = eval_expr("\"hello\"").expect("Failed to eval expr");
        assert_eq!(result, serde_json::json!("hello"));

        let result = eval_expr("{ a = 1; b = 2; }").expect("Failed to eval expr");
        assert_eq!(result, serde_json::json!({"a": 1, "b": 2}));
    }

    #[test]
    fn test_run_nix_eval() {
        let dir = tempdir().expect("Failed to create temp dir");
        let flake_lock = dir.path().join("flake.lock");
        std::fs::write(flake_lock, r#"{"nodes":{},"root":"root","version":7}"#).unwrap();

        // Create a minimal flake.nix that provides a "default" output
        let flake_nix = dir.path().join("flake.nix");
        std::fs::write(
            flake_nix,
            r#"
        {
          outputs = { self }: {
            default = "test";
            packages.x86_64-linux.default = "test";
            legacyPackages.x86_64-linux.default = "test";
          };
        }
        "#,
        )
        .unwrap();

        let options = EvalOptions {
            raw: true,
            ..Default::default()
        };

        match run_nix_eval(Some(dir.path()), "default", &options) {
            Ok(result) => assert_eq!(result, "test"),
            Err(e) => {
                let err_msg = e.to_string();
                if err_msg.contains("not found") || err_msg.contains("No such file or directory") {
                    println!("Warning: nix-instantiate not found, skipping full test");
                } else {
                    panic!("run_nix_eval failed: {}", e);
                }
            }
        }
    }

    #[test]
    fn test_get_lock_expr() {
        let dir = tempdir().expect("Failed to create temp dir");
        // No lock file
        let expr = get_lock_expr(dir.path());
        assert!(expr.contains("nodes = { root = { inputs = {}; }; };"));

        // With lock file
        let flake_lock = dir.path().join("flake.lock");
        std::fs::write(flake_lock, r#"{"version":7}"#).unwrap();
        let expr = get_lock_expr(dir.path());
        assert!(expr.contains("builtins.fromJSON (builtins.readFile"));
    }

    pub fn eval_expr(expr: &str) -> Result<serde_json::Value> {
        let mut cmd = crate::command::NixCommand::new("nix-instantiate");
        cmd.args(["--eval", "--json", "--expr", expr]);
        cmd.json()
    }
}
