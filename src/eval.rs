//! Nix expression evaluation using subprocess commands.
//!
//! This module provides flake evaluation by shelling out to `nix` commands:
//! - `nix eval --impure --expr` for evaluation (never enters flake context)
//! - `nix build <drv>^*` for building from .drv paths
//! - `nix flake prefetch` for fetching inputs
//!
//! IMPORTANT: This module NEVER uses builtins.getFlake for local flakes,
//! as that would copy the flake to the nix store. Instead, we import
//! flake.nix directly and construct inputs from flake.lock manually.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::process::Command;

use anyhow::{anyhow, bail, Context, Result};
use tracing::{debug, instrument, trace};

use crate::lock::{FlakeLock, InputRef, LockedRef};

//=============================================================================
// Evaluation & Building
//=============================================================================

/// Information about a derivation, including which outputs to install.
#[derive(Debug, Clone)]
pub struct DrvInfo {
    /// Path to the .drv file in the nix store.
    pub drv_path: String,
    /// Which outputs to install (from meta.outputsToInstall, defaults to ["out"]).
    pub outputs_to_install: Vec<String>,
}

impl std::fmt::Display for DrvInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.drv_path)
    }
}

impl DrvInfo {
    /// Create a DrvInfo with default outputs (just "out").
    pub fn with_default_outputs(drv_path: String) -> Self {
        Self {
            drv_path,
            outputs_to_install: vec!["out".to_string()],
        }
    }
}

/// Evaluate a Nix expression and return the result as JSON.
///
/// Uses `nix eval --json --impure --expr` (never enters flake context).
#[instrument(level = "debug", skip(expr))]
pub fn eval_to_json(expr: &str) -> Result<serde_json::Value> {
    debug!("evaluating expression to JSON ({} bytes)", expr.len());
    trace!("expression:\n{}", expr);

    let output = Command::new("nix")
        .args(["eval", "--json", "--impure", "--expr", expr])
        .output()
        .context("failed to execute nix eval")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        debug!("nix eval failed with stderr:\n{}", stderr);
        bail!("evaluation failed: {}", stderr.trim());
    }

    let json_str = String::from_utf8_lossy(&output.stdout);
    let value = serde_json::from_str(&json_str)
        .context("failed to parse JSON output")?;

    Ok(value)
}

/// Try to get the program path from an app attribute.
///
/// Apps have a `.program` attribute that points to an executable.
/// Returns None if the attribute is not an app.
#[instrument(level = "debug", skip(lock))]
pub fn try_get_app_program(
    flake_path: &Path,
    lock: &FlakeLock,
    attr_path: &[String],
    input_overrides: &HashMap<String, String>,
) -> Result<Option<String>> {
    debug!("checking if {} is an app", attr_path.join("."));

    // Prefetch inputs (same as for eval)
    let store_paths = if input_overrides.is_empty() {
        prefetch_all_inputs(lock)?
    } else {
        let nodes_to_fetch: Vec<_> = lock.nodes.iter()
            .filter(|(name, _)| *name != &lock.root && !input_overrides.contains_key(*name))
            .collect();

        let mut paths = HashMap::new();
        for (name, node) in nodes_to_fetch {
            if let Some(ref locked) = node.locked {
                let store_path = prefetch_input(name, locked)?;
                paths.insert(name.clone(), store_path);
            }
        }
        paths
    };

    let flake_dir = flake_path.to_str()
        .ok_or_else(|| anyhow!("invalid flake path"))?;

    // Generate expression that tries to get .program attribute
    let mut program_attr = attr_path.to_vec();
    program_attr.push("program".to_string());

    let expr = generate_flake_eval_expr(
        flake_dir,
        lock,
        &program_attr,
        input_overrides,
        &store_paths,
    )?;

    // Try to evaluate - if it fails, it's not an app
    match eval_to_json(&expr) {
        Ok(json) => {
            if let Some(program) = json.as_str() {
                debug!("found app program: {}", program);
                Ok(Some(program.to_string()))
            } else {
                Ok(None)
            }
        }
        Err(_) => Ok(None), // Not an app
    }
}

/// Get the main program name from a derivation.
///
/// Tries in order: meta.mainProgram, pname, name (with version stripped).
#[instrument(level = "debug", skip(lock))]
pub fn get_main_program(
    flake_path: &Path,
    lock: &FlakeLock,
    attr_path: &[String],
    input_overrides: &HashMap<String, String>,
    fallback: &str,
) -> Result<String> {
    debug!("getting main program for {}", attr_path.join("."));

    // Prefetch inputs
    let store_paths = if input_overrides.is_empty() {
        prefetch_all_inputs(lock)?
    } else {
        let nodes_to_fetch: Vec<_> = lock.nodes.iter()
            .filter(|(name, _)| *name != &lock.root && !input_overrides.contains_key(*name))
            .collect();

        let mut paths = HashMap::new();
        for (name, node) in nodes_to_fetch {
            if let Some(ref locked) = node.locked {
                let store_path = prefetch_input(name, locked)?;
                paths.insert(name.clone(), store_path);
            }
        }
        paths
    };

    let flake_dir = flake_path.to_str()
        .ok_or_else(|| anyhow!("invalid flake path"))?;

    // Try meta.mainProgram
    let mut meta_main = attr_path.to_vec();
    meta_main.extend(vec!["meta".to_string(), "mainProgram".to_string()]);
    let expr = generate_flake_eval_expr(flake_dir, lock, &meta_main, input_overrides, &store_paths)?;
    if let Ok(json) = eval_to_json(&expr) {
        if let Some(s) = json.as_str() {
            debug!("found meta.mainProgram: {}", s);
            return Ok(s.to_string());
        }
    }

    // Try pname
    let mut pname_attr = attr_path.to_vec();
    pname_attr.push("pname".to_string());
    let expr = generate_flake_eval_expr(flake_dir, lock, &pname_attr, input_overrides, &store_paths)?;
    if let Ok(json) = eval_to_json(&expr) {
        if let Some(s) = json.as_str() {
            debug!("using pname as mainProgram: {}", s);
            return Ok(s.to_string());
        }
    }

    // Try name (strip version)
    let mut name_attr = attr_path.to_vec();
    name_attr.push("name".to_string());
    let expr = generate_flake_eval_expr(flake_dir, lock, &name_attr, input_overrides, &store_paths)?;
    if let Ok(json) = eval_to_json(&expr) {
        if let Some(name) = json.as_str() {
            // Strip version suffix (last -X.Y.Z part)
            if let Some(pos) = name.rfind('-') {
                let suffix = &name[pos + 1..];
                if suffix.chars().next().map_or(false, |c| c.is_ascii_digit()) {
                    let base = &name[..pos];
                    debug!("using name (stripped version) as mainProgram: {}", base);
                    return Ok(base.to_string());
                }
            }
            debug!("using name as mainProgram: {}", name);
            return Ok(name.to_string());
        }
    }

    // Fallback
    debug!("using fallback as mainProgram: {}", fallback);
    Ok(fallback.to_string())
}

/// Evaluate a Nix expression to a derivation path (.drv file).
///
/// Evaluates `.drvPath` via `nix eval --impure` to instantiate the derivation.
#[instrument(level = "debug", skip(expr), fields(expr_len = expr.len()))]
pub fn eval_to_drv(expr: &str, source_name: &str) -> Result<String> {
    debug!("evaluating expression to .drv ({} bytes)", expr.len());
    trace!("expression source: {}", source_name);
    trace!("full expression:\n{}", expr);

    // Evaluate .drvPath to instantiate the derivation and get its store path.
    // --impure is needed for builtins.currentSystem and file imports.
    // --raw outputs the string without JSON quoting.
    let drv_expr = format!("({}).drvPath", expr);
    let output = Command::new("nix")
        .args(["eval", "--raw", "--impure", "--expr", &drv_expr])
        .output()
        .context("failed to execute nix eval")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        debug!("nix eval failed with stderr:\n{}", stderr);
        bail!("evaluation failed: {}", stderr.trim());
    }

    let drv_path = String::from_utf8_lossy(&output.stdout).trim().to_string();
    debug!("evaluated to derivation: {}", drv_path);

    if !drv_path.ends_with(".drv") {
        bail!("nix eval returned invalid path (expected .drv): {}", drv_path);
    }

    Ok(drv_path)
}

/// Evaluate a Nix expression to DrvInfo (drv path + outputsToInstall).
///
/// This evaluates both `.drvPath` and `.meta.outputsToInstall` to determine
/// which outputs should be built, matching `nix build` behavior.
#[instrument(level = "debug", skip(expr), fields(expr_len = expr.len()))]
pub fn eval_to_drv_info(expr: &str, source_name: &str) -> Result<DrvInfo> {
    debug!("evaluating expression to DrvInfo ({} bytes)", expr.len());
    trace!("expression source: {}", source_name);
    trace!("full expression:\n{}", expr);

    // Evaluate both drvPath and meta.outputsToInstall in a single nix eval call.
    // This matches how `nix build` determines which outputs to build.
    let info_expr = format!(
        r#"let drv = ({}); in builtins.toJSON {{
            drvPath = drv.drvPath;
            outputsToInstall = drv.meta.outputsToInstall or [ drv.outputName or "out" ];
        }}"#,
        expr
    );

    let output = Command::new("nix")
        .args(["eval", "--raw", "--impure", "--expr", &info_expr])
        .output()
        .context("failed to execute nix eval")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        debug!("nix eval failed with stderr:\n{}", stderr);
        bail!("evaluation failed: {}", stderr.trim());
    }

    let json_str = String::from_utf8_lossy(&output.stdout);
    let info: serde_json::Value = serde_json::from_str(&json_str)
        .context("failed to parse DrvInfo JSON")?;

    let drv_path = info["drvPath"]
        .as_str()
        .ok_or_else(|| anyhow!("missing drvPath in eval result"))?
        .to_string();

    let outputs_to_install: Vec<String> = info["outputsToInstall"]
        .as_array()
        .ok_or_else(|| anyhow!("missing outputsToInstall in eval result"))?
        .iter()
        .filter_map(|v| v.as_str().map(|s| s.to_string()))
        .collect();

    debug!("evaluated to derivation: {}, outputs: {:?}", drv_path, outputs_to_install);

    if !drv_path.ends_with(".drv") {
        bail!("nix eval returned invalid path (expected .drv): {}", drv_path);
    }

    if outputs_to_install.is_empty() {
        bail!("outputsToInstall is empty");
    }

    Ok(DrvInfo {
        drv_path,
        outputs_to_install,
    })
}

/// Build a derivation and return the output path.
///
/// Uses `nix build` with a .drv store path (no flake context).
/// Builds only the specified outputs (from meta.outputsToInstall).
/// Returns the first output path.
#[instrument(level = "debug", fields(drv = %drv_path, outputs = ?outputs))]
pub fn build_drv(drv_path: &str, outputs: &[String]) -> Result<String> {
    debug!("building derivation with nix build");

    // Build the derivation directly from its store path.
    // Use ^output1,output2,... syntax to build only the specified outputs.
    // This matches `nix build` behavior which uses meta.outputsToInstall.
    let outputs_suffix = if outputs.is_empty() {
        "out".to_string() // fallback to "out" if no outputs specified
    } else {
        outputs.join(",")
    };
    let installable = format!("{}^{}", drv_path, outputs_suffix);

    let output = Command::new("nix")
        .args(["build", &installable, "--no-link", "--print-out-paths"])
        .output()
        .context("failed to execute nix build")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        debug!("nix build failed with stderr:\n{}", stderr);
        bail!("build failed: {}", stderr.trim());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);

    // nix build --print-out-paths outputs one path per line.
    // Return the first output path (typically "out").
    let output_path = stdout
        .lines()
        .next()
        .ok_or_else(|| anyhow!("nix build produced no output"))?
        .trim()
        .to_string();

    debug!("build completed: {}", output_path);

    if !output_path.starts_with("/nix/store/") {
        bail!("nix build returned invalid output path: {}", output_path);
    }

    Ok(output_path)
}

/// Build a derivation and return all output paths.
///
/// For derivations with multiple outputs (out, dev, doc, etc.).
#[instrument(level = "debug", fields(drv = %drv_path))]
pub fn build_drv_outputs(drv_path: &str) -> Result<HashMap<String, String>> {
    debug!("building derivation with nix build");

    let installable = format!("{}^*", drv_path);
    let output = Command::new("nix")
        .args(["build", &installable, "--no-link", "--print-out-paths"])
        .output()
        .context("failed to execute nix build")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        debug!("nix build failed with stderr:\n{}", stderr);
        bail!("build failed: {}", stderr.trim());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);

    // nix build --print-out-paths outputs one path per line
    let mut outputs = HashMap::new();

    for (idx, line) in stdout.lines().enumerate() {
        let path = line.trim();
        if path.is_empty() {
            continue;
        }

        if !path.starts_with("/nix/store/") {
            bail!("nix build returned invalid output path: {}", path);
        }

        // First output is "out", subsequent ones get indexed names
        let output_name = if idx == 0 { "out" } else { &format!("out{}", idx) };
        outputs.insert(output_name.to_string(), path.to_string());
    }

    if outputs.is_empty() {
        bail!("nix build produced no output paths");
    }

    debug!("build completed with {} outputs", outputs.len());
    Ok(outputs)
}

//=============================================================================
// Input Fetching via Subprocess
//=============================================================================

/// Prefetch a flake input to the store using `nix flake prefetch`.
///
/// Returns the store path where the input was fetched.
/// This respects access-tokens from nix.conf automatically.
#[instrument(level = "debug", fields(input_name = %input_name))]
pub fn prefetch_input(input_name: &str, locked: &LockedRef) -> Result<String> {
    debug!("prefetching input: {}", input_name);

    // Convert LockedRef to a flake reference string
    let flake_ref = locked_ref_to_flake_ref(locked)?;

    trace!("flake reference: {}", flake_ref);

    // Call nix flake prefetch
    let output = Command::new("nix")
        .args(["--extra-experimental-features", "nix-command flakes", "flake", "prefetch", "--json", &flake_ref])
        .output()
        .context("failed to execute nix flake prefetch")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        debug!("nix flake prefetch failed with stderr:\n{}", stderr);
        bail!("failed to prefetch input '{}': {}", input_name, stderr.trim());
    }

    // Parse JSON output to get store path
    let json_str = String::from_utf8_lossy(&output.stdout);
    let info: serde_json::Value = serde_json::from_str(&json_str)
        .context("failed to parse nix flake prefetch JSON output")?;

    let store_path = info["storePath"]
        .as_str()
        .ok_or_else(|| anyhow!("nix flake prefetch JSON missing storePath field"))?
        .to_string();

    debug!("prefetched {} to: {}", input_name, store_path);
    Ok(store_path)
}

/// Convert a LockedRef to a flake reference string for `nix flake prefetch`.
///
/// Examples:
/// - GitHub → "github:owner/repo/rev"
/// - Git → "git+url?rev=..."
/// - Tarball → "tarball+url"
fn locked_ref_to_flake_ref(locked: &LockedRef) -> Result<String> {
    match locked {
        LockedRef::GitHub { owner, repo, rev, .. } => {
            Ok(format!("github:{}/{}?rev={}", owner, repo, rev))
        }
        LockedRef::GitLab { owner, repo, rev, .. } => {
            Ok(format!("gitlab:{}/{}?rev={}", owner, repo, rev))
        }
        LockedRef::Sourcehut { owner, repo, rev, .. } => {
            Ok(format!("sourcehut:{}/{}?rev={}", owner, repo, rev))
        }
        LockedRef::Git { url, rev, dirty_rev, .. } => {
            let effective_rev = rev.as_ref().or(dirty_rev.as_ref())
                .ok_or_else(|| anyhow!("git input missing rev"))?;
            Ok(format!("git+{}?rev={}", url, effective_rev))
        }
        LockedRef::Tarball { url, .. } => {
            Ok(format!("tarball+{}", url))
        }
        LockedRef::Path { path, .. } => {
            // For local paths, we can add them to the store using nix store add-path
            // But actually, for local paths we don't need to prefetch - we can use them directly
            Ok(path.clone())
        }
        LockedRef::Indirect { id, .. } => {
            Err(anyhow!("indirect input '{}' should be resolved before locking", id))
        }
    }
}

/// Prefetch all inputs from a flake.lock file.
///
/// Returns a map of input_name → store_path.
/// This parallelizes prefetching for performance.
#[instrument(level = "debug", skip(lock))]
pub fn prefetch_all_inputs(lock: &FlakeLock) -> Result<HashMap<String, String>> {
    use rayon::prelude::*;

    debug!("prefetching all inputs");

    // Get all nodes except root
    let nodes_to_fetch: Vec<_> = lock.nodes.iter()
        .filter(|(name, _)| *name != &lock.root)
        .collect();

    debug!("found {} inputs to prefetch", nodes_to_fetch.len());

    // Prefetch in parallel using rayon
    let results: Vec<_> = nodes_to_fetch.par_iter()
        .filter_map(|(name, node)| {
            if let Some(ref locked) = node.locked {
                match prefetch_input(name, locked) {
                    Ok(store_path) => Some(Ok(((*name).clone(), store_path))),
                    Err(e) => Some(Err(anyhow!("failed to prefetch {}: {}", name, e))),
                }
            } else {
                None
            }
        })
        .collect();

    // Convert Vec<Result<(String, String)>> to Result<HashMap<String, String>>
    let mut store_paths = HashMap::new();
    for result in results {
        let (name, path) = result?;
        store_paths.insert(name, path);
    }

    debug!("prefetched {} inputs successfully", store_paths.len());
    Ok(store_paths)
}

//=============================================================================
// Expression Generation for Local Flakes
// (Preserves the core value proposition - never copies local flakes to store!)
//=============================================================================

/// Generate a Nix expression that evaluates a local flake's attribute.
///
/// This is the core of trix's value proposition. We:
/// 1. Import flake.nix directly (NOT via builtins.getFlake)
/// 2. Prefetch all inputs from flake.lock
/// 3. Construct the inputs attrset manually
/// 4. Call the flake's outputs function
///
/// This ensures the local flake is NEVER copied to /nix/store.
/// Returns DrvInfo containing both the drv path and outputs to install.
#[instrument(level = "debug", skip(lock), fields(attr = ?attr_path))]
pub fn generate_and_eval_local_flake(
    flake_path: &Path,
    lock: &FlakeLock,
    attr_path: &[String],
    input_overrides: &HashMap<String, String>,
) -> Result<DrvInfo> {
    debug!("generating expression for local flake: {}", flake_path.display());

    // Step 1: Prefetch all inputs to the store
    let store_paths = if input_overrides.is_empty() {
        prefetch_all_inputs(lock)?
    } else {
        // If we have overrides, we need to be more selective about what we prefetch
        // Overridden inputs should not be prefetched
        let nodes_to_fetch: Vec<_> = lock.nodes.iter()
            .filter(|(name, _)| *name != &lock.root && !input_overrides.contains_key(*name))
            .collect();

        let mut paths = HashMap::new();
        for (name, node) in nodes_to_fetch {
            if let Some(ref locked) = node.locked {
                let store_path = prefetch_input(name, locked)?;
                paths.insert(name.clone(), store_path);
            }
        }
        paths
    };

    // Step 2: Generate the expression using prefetched store paths
    let flake_dir = flake_path.to_str()
        .ok_or_else(|| anyhow!("invalid flake path"))?;

    let expr = generate_flake_eval_expr(
        flake_dir,
        lock,
        attr_path,
        input_overrides,
        &store_paths,
    )?;

    // Step 3: Evaluate the expression to get .drv path and outputsToInstall
    let source_name = format!("{}#{}", flake_path.display(), attr_path.join("."));
    eval_to_drv_info(&expr, &source_name)
}

/// Generate a Nix expression for evaluating a local flake attribute.
///
/// This generates an expression that:
/// - Uses prefetched store paths for inputs (already fetched via subprocess)
/// - Imports flake.nix directly
/// - Constructs inputs manually
/// - Never copies the local flake to the store
///
/// This is the SAME algorithm as the old evaluator, just with prefetched paths.
pub fn generate_flake_eval_expr(
    flake_dir: &str,
    lock: &FlakeLock,
    attr_path: &[String],
    input_overrides: &HashMap<String, String>,
    store_paths: &HashMap<String, String>,
) -> Result<String> {
    // Get root node's inputs
    let root_node = lock.nodes.get(&lock.root);
    let root_inputs: HashMap<String, InputRef> = root_node
        .map(|n| n.inputs.clone())
        .unwrap_or_default();

    // Build topologically sorted list of nodes (dependencies first)
    let sorted_nodes = topological_sort_nodes(lock)?;

    // Generate let bindings for each input
    let mut let_bindings = Vec::new();

    for node_name in &sorted_nodes {
        if node_name == &lock.root {
            continue; // Skip root
        }

        let node = lock.nodes.get(node_name)
            .ok_or_else(|| anyhow!("node '{}' not found in lock", node_name))?;

        // Check if this input is overridden with a local path
        if let Some(override_path) = input_overrides.get(node_name) {
            // Resolve the override path (handle ~ and relative paths)
            let resolved_path = resolve_override_path(override_path)?;

            // Generate expression for overridden input (local path, no store copy)
            // The override path has a flake.nix (verified by resolve_override_path),
            // so always treat it as a flake regardless of the lock file's original metadata.
            let override_is_flake = Path::new(&resolved_path).join("flake.nix").exists();
            let override_expr = generate_override_input_expr(
                node_name,
                &resolved_path,
                override_is_flake,
                lock,
                store_paths,
            )?;
            let_bindings.push(override_expr);
            continue;
        }

        // Use the prefetched store path
        let store_path = store_paths.get(node_name)
            .ok_or_else(|| anyhow!("input '{}' not prefetched", node_name))?;

        // If it's a flake, generate the input building expression
        if node.flake {
            let input_expr = generate_input_build_expr_from_store_path(
                node_name,
                store_path,
                node,
                lock,
            )?;
            let_bindings.push(format!("{} = {};", sanitize_name(node_name), input_expr));
        } else {
            // Non-flake input - just use the store path
            let_bindings.push(format!(
                "{name} = {{ outPath = \"{path}\"; }};",
                name = sanitize_name(node_name),
                path = store_path,
            ));
        }
    }

    // Build the root inputs attrset
    let mut input_attrs = Vec::new();
    let mut resolved_root_inputs: Vec<(String, String)> = Vec::new();
    for (input_name, input_ref) in &root_inputs {
        let resolved_name = match input_ref {
            InputRef::Direct(name) => sanitize_name(name),
            InputRef::Follows(path) => {
                // Follows at root level - resolve to the target
                match resolve_follows_to_name(path, lock)? {
                    FollowsResolution::Node(name) => name,
                    FollowsResolution::Self_ => "self".to_string(),
                }
            }
        };
        // Quote the attribute name to preserve hyphens
        input_attrs.push(format!("\"{}\" = {};", input_name, resolved_name));
        resolved_root_inputs.push((input_name.clone(), resolved_name));
    }

    // Build the outputs call arguments
    let mut output_args = vec!["self = self".to_string()];
    for (input_name, sanitized) in &resolved_root_inputs {
        output_args.push(format!("\"{}\" = {}", input_name, sanitized));
    }

    // Generate the final expression
    let attr_suffix = if attr_path.is_empty() {
        String::new()
    } else {
        format!(".{}", attr_path.join("."))
    };

    // Get git metadata without copying to store
    let git_attrs = get_git_metadata(flake_dir);

    let expr = format!(
        r#"
let
  # Use string concatenation to avoid copying to store
  flakeDirPath = "{flake_dir}";

  # Minimal self for nested inputs that follow root
  _rootSelf = {{
    outPath = flakeDirPath;  # String, not path - won't trigger store copy
    _type = "flake";
    {git_attrs}
  }};

  # Built inputs (from prefetched store paths)
  {let_bindings}

  # Self input (the local flake) with full inputs
  self = _rootSelf // {{
    inputs = {{ {input_attrs} }};
  }};

  # Import and evaluate the flake
  # Keep as string until import, avoid path coercion
  flake = import ((toString flakeDirPath) + "/flake.nix");
  outputs = flake.outputs ({{ {output_args}; }} // {{ self = self // outputs; }});

in outputs{attr_suffix}
"#,
        flake_dir = flake_dir,
        let_bindings = let_bindings.join("\n  "),
        input_attrs = input_attrs.join(" "),
        git_attrs = git_attrs,
        output_args = output_args.join("; "),
        attr_suffix = attr_suffix,
    );

    Ok(expr)
}

//=============================================================================
// Helper Functions for Expression Generation
// (Preserved from the original evaluator)
//=============================================================================

/// Generate expression to build an input from its prefetched store path.
fn generate_input_build_expr_from_store_path(
    _node_name: &str,
    store_path: &str,
    node: &crate::lock::LockNode,
    lock: &FlakeLock,
) -> Result<String> {
    // Build this input's inputs
    let mut input_exprs = Vec::new();
    for (input_name, input_ref) in &node.inputs {
        let resolved = match input_ref {
            InputRef::Direct(name) => sanitize_name(name),
            InputRef::Follows(path) => {
                match resolve_follows_to_name(path, lock)? {
                    FollowsResolution::Node(name) => name,
                    FollowsResolution::Self_ => "_rootSelf".to_string(),
                }
            }
        };
        input_exprs.push(format!("\"{}\" = {};", input_name, resolved));
    }

    let inputs_str = input_exprs.join(" ");

    // Get metadata (rev, shortRev, lastModified, lastModifiedDate) from locked ref
    let metadata = node.locked.as_ref()
        .map(|l| get_locked_ref_metadata(l))
        .unwrap_or_default();

    Ok(format!(
        r#"let
    _flake = import ((toString "{store_path}") + "/flake.nix");
    _inputs = {{ {inputs} }};
    _self = {{ outPath = "{store_path}"; inputs = _inputs; _type = "flake";{metadata} }};
    _outputs = _flake.outputs (_inputs // {{ self = _self // _outputs; }});
  in _outputs // {{ outPath = "{store_path}"; inputs = _inputs; outputs = _outputs; _type = "flake";{metadata} }}"#,
        store_path = store_path,
        inputs = inputs_str,
        metadata = metadata,
    ))
}

/// Topologically sort lock nodes (dependencies first).
fn topological_sort_nodes(lock: &FlakeLock) -> Result<Vec<String>> {
    let mut sorted = Vec::new();
    let mut visited = HashSet::new();
    let mut in_progress = HashSet::new();

    fn visit(
        node_name: &str,
        lock: &FlakeLock,
        sorted: &mut Vec<String>,
        visited: &mut HashSet<String>,
        in_progress: &mut HashSet<String>,
    ) -> Result<()> {
        if visited.contains(node_name) {
            return Ok(());
        }
        if in_progress.contains(node_name) {
            return Err(anyhow!("circular dependency detected at '{}'", node_name));
        }

        in_progress.insert(node_name.to_string());

        if let Some(node) = lock.nodes.get(node_name) {
            for (_, input_ref) in &node.inputs {
                let dep_name = match input_ref {
                    InputRef::Direct(name) => name.clone(),
                    InputRef::Follows(path) => {
                        if path.is_empty() {
                            continue;
                        }
                        match resolve_follows_to_node_name(path, lock) {
                            Ok(Some(name)) => name,
                            Ok(None) => continue,
                            Err(_) => continue,
                        }
                    }
                };
                visit(&dep_name, lock, sorted, visited, in_progress)?;
            }
        }

        in_progress.remove(node_name);
        visited.insert(node_name.to_string());
        sorted.push(node_name.to_string());

        Ok(())
    }

    // Start from root and visit all nodes
    if let Some(root_node) = lock.nodes.get(&lock.root) {
        for (_, input_ref) in &root_node.inputs {
            let node_name = match input_ref {
                InputRef::Direct(name) => name,
                InputRef::Follows(_) => continue,
            };
            visit(node_name, lock, &mut sorted, &mut visited, &mut in_progress)?;
        }
    }

    Ok(sorted)
}

/// Resolve a follows path to the original node name (not sanitized).
fn resolve_follows_to_node_name(path: &[String], lock: &FlakeLock) -> Result<Option<String>> {
    if path.is_empty() {
        return Ok(None);
    }

    let mut current = lock.root.clone();
    for segment in path {
        let node = lock.nodes.get(&current)
            .ok_or_else(|| anyhow!("node '{}' not found", current))?;
        match node.inputs.get(segment) {
            Some(InputRef::Direct(name)) => current = name.clone(),
            Some(InputRef::Follows(inner_path)) => {
                return resolve_follows_to_node_name(inner_path, lock);
            }
            None => return Ok(None),
        }
    }
    Ok(Some(current))
}

/// Resolve a follows path to a node name.
fn resolve_follows_to_name(path: &[String], lock: &FlakeLock) -> Result<FollowsResolution> {
    if path.is_empty() {
        return Ok(FollowsResolution::Self_);
    }

    let mut current = lock.root.clone();
    for segment in path {
        let node = lock.nodes.get(&current)
            .ok_or_else(|| anyhow!("node '{}' not found", current))?;
        match node.inputs.get(segment) {
            Some(InputRef::Direct(name)) => current = name.clone(),
            Some(InputRef::Follows(inner_path)) => {
                return resolve_follows_to_name(inner_path, lock);
            }
            None => return Err(anyhow!("input '{}' not found in node '{}'", segment, current)),
        }
    }
    Ok(FollowsResolution::Node(sanitize_name(&current)))
}

/// Result of resolving a follows path.
enum FollowsResolution {
    Node(String),
    Self_,
}

/// Sanitize a name for use as a Nix identifier.
fn sanitize_name(name: &str) -> String {
    name.replace('-', "_")
}

/// Extract metadata from a LockedRef.
///
/// Always provides rev, shortRev, lastModified, and lastModifiedDate to prevent
/// "attribute 'rev' missing" errors when flake outputs reference input metadata
/// (e.g., `nixpkgs.rev` in NixOS test derivations). For inputs without a real rev
/// (like tarballs), a zeroed-out placeholder is used.
fn get_locked_ref_metadata(locked: &crate::lock::LockedRef) -> String {
    use crate::lock::LockedRef;

    let (rev_opt, last_modified_opt) = match locked {
        LockedRef::GitHub { rev, last_modified, .. } => (Some(rev.clone()), *last_modified),
        LockedRef::GitLab { rev, .. } => (Some(rev.clone()), None),
        LockedRef::Sourcehut { rev, .. } => (Some(rev.clone()), None),
        LockedRef::Git { rev, dirty_rev, last_modified, .. } => {
            let effective_rev = rev.as_ref().or(dirty_rev.as_ref()).cloned();
            (effective_rev, *last_modified)
        }
        LockedRef::Path { last_modified, .. } => (None, *last_modified),
        LockedRef::Tarball { .. } | LockedRef::Indirect { .. } => (None, None),
    };

    let mut attrs = Vec::new();

    // Always provide rev and shortRev - use placeholder for inputs without a real rev
    let rev = rev_opt.unwrap_or_else(|| "0000000000000000000000000000000000000000".to_string());
    let short_rev = &rev[..7.min(rev.len())];
    attrs.push(format!(r#"rev = "{}";"#, rev));
    attrs.push(format!(r#"shortRev = "{}";"#, short_rev));

    // Always provide lastModified and lastModifiedDate
    let ts = last_modified_opt.unwrap_or(0);
    attrs.push(format!("lastModified = {};", ts));
    let datetime = chrono::DateTime::from_timestamp(ts as i64, 0)
        .unwrap_or_else(|| chrono::DateTime::UNIX_EPOCH);
    let date_str = datetime.format("%Y%m%d").to_string();
    attrs.push(format!(r#"lastModifiedDate = "{}";"#, date_str));

    format!(" {}", attrs.join(" "))
}

/// Get git metadata for a directory without copying to the nix store.
fn get_git_metadata(flake_dir: &str) -> String {
    let repo = match git2::Repository::discover(flake_dir) {
        Ok(r) => r,
        Err(_) => {
            return "lastModified = 0; lastModifiedDate = \"19700101\";".to_string();
        }
    };

    let head = match repo.head() {
        Ok(h) => h,
        Err(_) => {
            return "lastModified = 0; lastModifiedDate = \"19700101\";".to_string();
        }
    };

    let commit = match head.peel_to_commit() {
        Ok(c) => c,
        Err(_) => {
            return "lastModified = 0; lastModifiedDate = \"19700101\";".to_string();
        }
    };

    let rev = commit.id().to_string();
    let short_rev = &rev[..7.min(rev.len())];
    let timestamp = commit.time().seconds();

    let datetime = chrono::DateTime::from_timestamp(timestamp, 0)
        .unwrap_or_else(|| chrono::DateTime::UNIX_EPOCH);
    let date_str = datetime.format("%Y%m%d").to_string();

    format!(
        r#"rev = "{}"; shortRev = "{}"; lastModified = {}; lastModifiedDate = "{}";"#,
        rev, short_rev, timestamp, date_str
    )
}

/// Resolve an override path, handling ~ expansion and converting to absolute path.
fn resolve_override_path(path: &str) -> Result<String> {
    let expanded = if path.starts_with("~/") {
        let home = std::env::var("HOME")
            .context("HOME environment variable not set")?;
        format!("{}{}", home, &path[1..])
    } else if path.starts_with('~') {
        return Err(anyhow!("~user paths are not supported, use absolute path or ~/"));
    } else {
        path.to_string()
    };

    // Convert to absolute path if relative
    let abs_path = if expanded.starts_with('/') {
        expanded
    } else {
        let cwd = std::env::current_dir()
            .context("failed to get current directory")?;
        cwd.join(&expanded)
            .canonicalize()
            .with_context(|| format!("override path does not exist: {}", expanded))?
            .to_string_lossy()
            .to_string()
    };

    // Verify the path exists and has a flake.nix
    let flake_nix = Path::new(&abs_path).join("flake.nix");
    if !flake_nix.exists() {
        return Err(anyhow!(
            "override path '{}' does not contain a flake.nix",
            abs_path
        ));
    }

    Ok(abs_path)
}

/// Generate a Nix expression for an overridden input (local path, no store copy).
fn generate_override_input_expr(
    node_name: &str,
    override_path: &str,
    is_flake: bool,
    _lock: &FlakeLock,
    _store_paths: &HashMap<String, String>,
) -> Result<String> {
    let sanitized = sanitize_name(node_name);

    if !is_flake {
        // Non-flake override - just use direct path
        return Ok(format!(
            "{name} = {{ outPath = \"{path}\"; }};",
            name = sanitized,
            path = override_path
        ));
    }

    // For flake overrides, generate similar expression to main flake
    // TODO: Handle override's own inputs (would need to parse its flake.lock)
    // For now, assume no inputs or handle later

    let git_attrs = get_git_metadata(override_path);

    Ok(format!(
        r#"# Overridden input: {node_name} -> {override_path}
  {name} = let
    _override_path = "{path}";
    _flake = import ((toString _override_path) + "/flake.nix");
    _inputs = {{ }};  # TODO: Handle override's inputs
    _self = {{ outPath = _override_path; inputs = _inputs; _type = "flake"; {git_attrs} }};
    _outputs = _flake.outputs (_inputs // {{ self = _self // _outputs; }});
  in _outputs // {{ outPath = _override_path; inputs = _inputs; outputs = _outputs; _type = "flake"; }};"#,
        node_name = node_name,
        override_path = override_path,
        name = sanitized,
        path = override_path,
        git_attrs = git_attrs,
    ))
}

//=============================================================================
// Flake Show / Check Evaluation
// (Native evaluation of flake outputs structure without store copy)
//=============================================================================

/// Evaluate a Nix expression by wrapping it in builtins.toJSON, avoiding --strict.
///
/// This approach serializes to JSON within the Nix evaluation context, where
/// builtins.tryEval wrappers are active. The --strict flag forces evaluation in
/// a separate phase outside tryEval contexts, which can trigger errors from lazy
/// thunks that reference missing attributes (e.g., nixpkgs.rev on tarball inputs).
fn eval_to_json_via_tojson(expr: &str) -> Result<serde_json::Value> {
    debug!("evaluating expression via builtins.toJSON ({} bytes)", expr.len());
    trace!("expression:\n{}", expr);

    // Wrap the expression in builtins.toJSON so serialization happens during
    // evaluation (where tryEval is active), not during --strict post-processing.
    let wrapped = format!("builtins.toJSON ({})", expr);

    let output = Command::new("nix")
        .args(["eval", "--json", "--impure", "--expr", &wrapped])
        .output()
        .context("failed to execute nix eval")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        debug!("nix eval (toJSON) failed with stderr:\n{}", stderr);
        bail!("evaluation failed: {}", stderr.trim());
    }

    // Output is a JSON-encoded string (double-encoded JSON).
    // First parse as JSON to get the inner string, then parse that as JSON.
    let outer_json: String = serde_json::from_slice(&output.stdout)
        .context("failed to parse outer JSON string from nix eval")?;

    let value: serde_json::Value = serde_json::from_str(&outer_json)
        .context("failed to parse inner JSON from builtins.toJSON output")?;

    Ok(value)
}

/// Evaluate flake outputs structure for `trix flake show --json`.
///
/// Produces output matching `nix flake show --json` format without copying
/// the local flake to the store.
#[instrument(level = "debug", skip(lock))]
pub fn eval_flake_show_json(
    flake_path: &Path,
    lock: &FlakeLock,
    all_systems: bool,
    legacy: bool,
) -> Result<serde_json::Value> {
    debug!("evaluating flake outputs for show: {}", flake_path.display());

    // Prefetch all inputs
    let store_paths = prefetch_all_inputs(lock)?;

    let flake_dir = flake_path.to_str()
        .ok_or_else(|| anyhow!("invalid flake path"))?;

    // Generate the base expression (ends with "in outputs")
    let base_expr = generate_flake_eval_expr(
        flake_dir,
        lock,
        &[],
        &HashMap::new(),
        &store_paths,
    )?;

    let all_systems_nix = if all_systems { "true" } else { "false" };
    let show_legacy_nix = if legacy { "true" } else { "false" };

    // Wrap with the show expression that produces nix flake show --json format
    let show_expr = format!(
        r#"
let
  outputs = ({base_expr});
  allSystemsFlag = {all_systems_nix};
  showLegacyFlag = {show_legacy_nix};
  currentSystem = builtins.currentSystem;

  perSystemAttrs = ["packages" "devShells" "checks" "apps"];

  # Get derivation/app info. tryEval protects against derivations whose
  # arguments reference missing attributes.
  getDrvInfo = drv:
    if builtins.isAttrs drv && (drv.type or null) == "derivation" then
      let
        nameResult = builtins.tryEval (builtins.seq drv.name drv.name);
        descResult = builtins.tryEval (drv.meta.description or "");
        name = if nameResult.success then nameResult.value else "unknown";
        desc = if descResult.success && descResult.value != null then descResult.value else "";
      in {{ type = "derivation"; name = name; description = desc; }}
    else if builtins.isAttrs drv && drv ? type && drv.type == "app" then
      let
        descResult = builtins.tryEval (
          if drv ? meta.description then drv.meta.description
          else if drv ? description then drv.description
          else ""
        );
        desc = if descResult.success && descResult.value != null then descResult.value else "";
      in {{ type = "app"; }} // (if desc != "" then {{ description = desc; }} else {{}})
    else {{}};

  # Recursively walk an attrset tree, detecting derivations at any depth.
  # Used for hydraJobs which can have varying nesting levels.
  walkTree = node:
    let r = builtins.tryEval (
      if builtins.isAttrs node then
        let info = getDrvInfo node;
        in if info != {{}} then info
        else builtins.mapAttrs (k: v:
          let r2 = builtins.tryEval (walkTree v);
          in if r2.success then r2.value else {{}}
        ) node
      else {{}}
    ); in if r.success then r.value else {{}};

  # Process a per-system category (packages, devShells, checks, apps).
  # Non-current systems show attr names with {{}} values.
  processPerSystem = cat: val:
    let
      raw = builtins.mapAttrs (sys: sysVal:
        let result = builtins.tryEval (
          if builtins.isAttrs sysVal then
            if (sys == currentSystem || allSystemsFlag || cat == "apps") then
              builtins.mapAttrs (name: drv:
                let r = builtins.tryEval (getDrvInfo drv);
                in if r.success then r.value else {{}}
              ) sysVal
            else
              builtins.mapAttrs (name: _: {{}}) sysVal
          else {{}}
        );
        in if result.success then result.value else {{}}
      ) val;
      nonEmpty = builtins.filter (name: raw.${{name}} != {{}}) (builtins.attrNames raw);
    in builtins.listToAttrs (map (name: {{ inherit name; value = raw.${{name}}; }}) nonEmpty);

  processLegacy = val:
    builtins.mapAttrs (sys: sysVal:
      if !showLegacyFlag then {{}}
      else if (sys == currentSystem || allSystemsFlag) then
        let result = builtins.tryEval (
          let
            names = builtins.attrNames sysVal;
            derivNames = builtins.filter (name:
              let r = builtins.tryEval (builtins.isAttrs sysVal.${{name}} && (sysVal.${{name}}.type or null) == "derivation");
              in r.success && r.value
            ) names;
          in builtins.listToAttrs (map (name:
            let r = builtins.tryEval (getDrvInfo sysVal.${{name}});
            in {{ inherit name; value = if r.success then r.value else {{}}; }}
          ) derivNames)
        );
        in if result.success then result.value else {{}}
      else {{}}
    ) val;

  processCategory = cat: val:
    let result = builtins.tryEval (
      if builtins.elem cat perSystemAttrs && builtins.isAttrs val then
        processPerSystem cat val
      else if cat == "legacyPackages" && builtins.isAttrs val then
        processLegacy val
      else if cat == "formatter" && builtins.isAttrs val then
        builtins.mapAttrs (sys: drv:
          if (sys == currentSystem || allSystemsFlag) then
            let r = builtins.tryEval (getDrvInfo drv);
            in if r.success then r.value else {{}}
          else {{}}
        ) val
      else if builtins.elem cat ["defaultPackage" "defaultApp" "devShell"] && builtins.isAttrs val then
        builtins.mapAttrs (sys: drv:
          if (sys == currentSystem || allSystemsFlag) then
            let r = builtins.tryEval (getDrvInfo drv);
            in if r.success then r.value else {{}}
          else {{}}
        ) val
      else if cat == "overlays" && builtins.isAttrs val then
        builtins.mapAttrs (name: _: {{ type = "nixpkgs-overlay"; }}) val
      else if cat == "overlay" then
        {{ type = "nixpkgs-overlay"; }}
      else if cat == "nixosModules" && builtins.isAttrs val then
        builtins.mapAttrs (name: _: {{ type = "nixos-module"; }}) val
      else if builtins.elem cat ["nixosModule" "darwinModule"] then
        {{ type = "nixos-module"; }}
      else if cat == "templates" && builtins.isAttrs val then
        builtins.mapAttrs (name: tmpl:
          if builtins.isAttrs tmpl && tmpl ? description then
            {{ type = "template"; description = tmpl.description or ""; }}
          else {{ type = "template"; }}
        ) val
      else if cat == "nixosConfigurations" && builtins.isAttrs val then
        builtins.mapAttrs (name: _: {{ type = "nixos-configuration"; }}) val
      else if cat == "hydraJobs" && builtins.isAttrs val then
        walkTree val
      else
        {{ type = "unknown"; }}
    );
    in if result.success then result.value else {{ type = "unknown"; }};

  categories = builtins.attrNames outputs;

  allResults = builtins.listToAttrs (map (cat: {{
    name = cat;
    value = processCategory cat outputs.${{cat}};
  }}) categories);

  nonEmptyCats = builtins.filter (name: allResults.${{name}} != {{}}) (builtins.attrNames allResults);

in builtins.listToAttrs (map (name: {{ inherit name; value = allResults.${{name}}; }}) nonEmptyCats)
"#,
        base_expr = base_expr,
        all_systems_nix = all_systems_nix,
        show_legacy_nix = show_legacy_nix,
    );

    eval_to_json_via_tojson(&show_expr)
}

/// Get the list of check derivation paths for a local flake.
///
/// Returns a list of (check_name, drv_path) for all checks in the current system.
#[instrument(level = "debug", skip(lock))]
pub fn eval_flake_checks(
    flake_path: &Path,
    lock: &FlakeLock,
    system: &str,
) -> Result<Vec<(String, String)>> {
    debug!("evaluating flake checks for system {}", system);

    // Prefetch all inputs
    let store_paths = prefetch_all_inputs(lock)?;

    let flake_dir = flake_path.to_str()
        .ok_or_else(|| anyhow!("invalid flake path"))?;

    // First, get the list of check names
    let names_expr = generate_flake_eval_expr(
        flake_dir,
        lock,
        &["checks".to_string(), system.to_string()],
        &HashMap::new(),
        &store_paths,
    )?;

    let check_names_expr = format!("builtins.attrNames ({})", names_expr);
    let names_json = eval_to_json(&check_names_expr)?;

    let check_names: Vec<String> = serde_json::from_value(names_json)
        .context("failed to parse check names")?;

    debug!("found {} checks: {:?}", check_names.len(), check_names);

    // Now evaluate each check to get its .drv path
    let mut results = Vec::new();
    for name in &check_names {
        let attr_path = vec![
            "checks".to_string(),
            system.to_string(),
            name.clone(),
        ];

        match generate_and_eval_local_flake(flake_path, lock, &attr_path, &HashMap::new()) {
            Ok(drv_info) => {
                results.push((name.clone(), drv_info.drv_path));
            }
            Err(e) => {
                debug!("failed to evaluate check {}: {}", name, e);
                // Continue with other checks
            }
        }
    }

    Ok(results)
}
