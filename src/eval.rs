//! Nix expression evaluation using nix-bindings.
//!
//! This module wraps the nix-bindings crate to provide high-level evaluation
//! functionality for flakes.
//!
//! IMPORTANT: This module NEVER uses builtins.getFlake for local flakes,
//! as that would copy the flake to the nix store. Instead, we import
//! flake.nix directly and construct inputs from flake.lock manually.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use anyhow::{anyhow, Context, Result};
use tracing::{debug, instrument, trace};

use std::collections::BTreeMap;

use nix_bindings_expr::eval_state::{
    gc_register_my_thread, init, EvalState, EvalStateBuilder, ThreadRegistrationGuard,
};
use nix_bindings_expr::value::ValueType;
use nix_bindings_fetchers::FetchersSettings;
use nix_bindings_flake::{
    EvalStateBuilderExt, FlakeLockFlags, FlakeReference, FlakeReferenceParseFlags, FlakeSettings,
    LockedFlake,
};
use nix_bindings_store::path::StorePath;
use nix_bindings_store::store::Store;
use serde_json::json;

use crate::lock::{FlakeLock, InputRef, LockedRef};

/// A wrapper around the Nix evaluator state with flake support.
pub struct Evaluator {
    eval_state: EvalState,
    /// Separate store handle for building (Store::realise requires &mut self).
    /// Cloned from eval_state's store - shares the same underlying connection.
    store: Store,
    /// Flake settings for native flake operations
    flake_settings: FlakeSettings,
    /// Fetcher settings for native flake operations
    fetchers_settings: FetchersSettings,
    _gc_guard: ThreadRegistrationGuard,
}

impl Evaluator {
    /// Initialize the Nix evaluator with flake support.
    ///
    /// This must be called once before any evaluation can occur.
    #[instrument(level = "debug", skip_all)]
    pub fn new() -> Result<Self> {
        debug!("initializing Nix evaluator");

        // Initialize the Nix library
        trace!("calling nix init()");
        init().context("failed to initialize Nix")?;

        // Register this thread with the Nix garbage collector
        trace!("registering GC thread");
        let gc_guard = gc_register_my_thread().context("failed to register GC thread")?;

        // Open the default store
        trace!("opening Nix store");
        let store = Store::open(None, HashMap::new()).context("failed to open Nix store")?;

        // Disable pure evaluation mode to allow adding paths to store during evaluation.
        // Without this, derivationStrict fails with "path not valid" for local paths.
        trace!("setting pure-eval to false");
        if let Err(e) = nix_bindings_util::settings::set("pure-eval", "false") {
            debug!("failed to set pure-eval (non-fatal): {:?}", e);
            // Continue anyway - the setting might already be false or this might not be critical
        }

        // Create flake settings to enable builtins.getFlake
        trace!("creating flake settings");
        let flake_settings = FlakeSettings::new().context("failed to create flake settings")?;

        // Create fetchers settings for remote flake operations
        trace!("creating fetchers settings");
        let fetchers_settings =
            FetchersSettings::new().context("failed to create fetchers settings")?;

        // Create the evaluation state with flake support
        trace!("building eval state");
        let eval_state = EvalStateBuilder::new(store)?
            .flakes(&flake_settings)?
            .build()
            .context("failed to create evaluation state")?;

        // Clone the store for building operations (realise requires &mut self)
        let store = eval_state.store().clone();

        debug!("Nix evaluator initialized");
        Ok(Self {
            eval_state,
            store,
            flake_settings,
            fetchers_settings,
            _gc_guard: gc_guard,
        })
    }

    /// Evaluate a Nix expression from a string.
    #[instrument(level = "trace", skip(self, expr), fields(source = %source_name, expr_len = expr.len()))]
    pub fn eval_string(&mut self, expr: &str, source_name: &str) -> Result<NixValue> {
        trace!("evaluating expression ({} bytes)", expr.len());
        let value = self
            .eval_state
            .eval_from_string(expr, source_name)
            .map_err(|e| anyhow!("evaluation error: {}", e))?;

        Ok(NixValue { inner: value })
    }

    /// Navigate to a nested attribute path within a value.
    #[instrument(level = "trace", skip(self, value), fields(path = ?path))]
    pub fn navigate_attr_path(&mut self, value: NixValue, path: &[String]) -> Result<NixValue> {
        let mut current = value;
        for attr in path {
            if attr.is_empty() {
                continue;
            }
            trace!("navigating to attr '{}'", attr);
            current = self
                .get_attr(&current, attr)?
                .ok_or_else(|| anyhow!("attribute '{}' not found", attr))?;
        }
        Ok(current)
    }

    /// Evaluate a flake's outputs WITHOUT copying the local flake to the store.
    ///
    /// This is the core of trix - we import flake.nix directly and construct
    /// inputs from flake.lock manually. Remote inputs are fetched normally
    /// (they get cached in the store, which is fine), but the local project
    /// is NEVER copied to the store.
    #[instrument(level = "debug", skip(self), fields(flake = %flake_path.display(), attr = ?attr_path))]
    pub fn eval_flake_attr(&mut self, flake_path: &Path, attr_path: &[String]) -> Result<NixValue> {
        self.eval_flake_attr_with_overrides(flake_path, attr_path, &HashMap::new())
    }

    /// Evaluate a flake's outputs with input overrides.
    ///
    /// Like `eval_flake_attr`, but allows overriding specific inputs with local paths.
    /// The overridden inputs are imported directly without copying to the store.
    #[instrument(level = "debug", skip(self, input_overrides), fields(flake = %flake_path.display(), attr = ?attr_path))]
    pub fn eval_flake_attr_with_overrides(
        &mut self,
        flake_path: &Path,
        attr_path: &[String],
        input_overrides: &HashMap<String, String>,
    ) -> Result<NixValue> {
        let path_str = flake_path
            .to_str()
            .ok_or_else(|| anyhow!("invalid flake path"))?;

        // Load and parse the lock file
        let lock_path = flake_path.join("flake.lock");
        let lock = if lock_path.exists() {
            debug!("reading flake.lock");
            let content = std::fs::read_to_string(&lock_path)
                .context("failed to read flake.lock")?;
            let lock: FlakeLock = serde_json::from_str(&content)
                .context("failed to parse flake.lock")?;
            debug!(nodes = lock.nodes.len(), "parsed flake.lock");
            lock
        } else {
            debug!("no flake.lock found, using empty lock");
            // Empty lock for flakes without inputs
            FlakeLock {
                nodes: HashMap::new(),
                root: "root".to_string(),
                version: 7,
            }
        };

        if !input_overrides.is_empty() {
            debug!(?input_overrides, "applying input overrides");
        }

        // Generate the evaluation expression
        debug!("generating eval expression");
        let expr = generate_flake_eval_expr(path_str, &lock, attr_path, input_overrides)?;
        trace!("generated expression:\n{}", expr);

        debug!("evaluating flake");
        self.eval_string(&expr, "<trix>")
    }

    /// Evaluate a flake's full outputs (for flake show, etc.)
    #[instrument(level = "debug", skip(self), fields(flake = %flake_path.display()))]
    pub fn eval_flake_outputs(&mut self, flake_path: &Path) -> Result<NixValue> {
        self.eval_flake_attr(flake_path, &[])
    }

    /// Evaluate a flake reference string natively using the Nix flake API.
    ///
    /// This handles ANY flake reference including:
    /// - Local paths: `.`, `./foo`, `/path/to/flake`
    /// - GitHub: `github:owner/repo`, `github:owner/repo/ref`
    /// - GitLab: `gitlab:owner/repo`
    /// - Sourcehut: `sourcehut:~owner/repo`
    /// - Git: `git+https://...`, `git+ssh://...`
    /// - Tarball: `https://example.com/foo.tar.gz`
    /// - Registry names: `nixpkgs`, `flake-utils`
    ///
    /// The flake reference can include a fragment for attribute path: `nixpkgs#hello`
    ///
    /// This method uses Nix's native flake resolution which:
    /// - Checks the registry for indirect references
    /// - Fetches remote flakes and caches them
    /// - Handles locking automatically
    #[instrument(level = "debug", skip(self), fields(flake_ref = %flake_ref))]
    pub fn eval_flake_ref(&mut self, flake_ref: &str, base_dir: &Path) -> Result<NixValue> {
        let base_dir_str = base_dir
            .to_str()
            .ok_or_else(|| anyhow!("invalid base directory path"))?;

        debug!("parsing flake reference: {}", flake_ref);

        // Create parse flags with base directory for relative paths
        let mut parse_flags = FlakeReferenceParseFlags::new(&self.flake_settings)
            .context("failed to create flake reference parse flags")?;
        parse_flags
            .set_base_directory(base_dir_str)
            .context("failed to set base directory")?;

        // Parse the flake reference (may include #fragment for attr path)
        let (flake_reference, fragment) = FlakeReference::parse_with_fragment(
            &self.fetchers_settings,
            &self.flake_settings,
            &parse_flags,
            flake_ref,
        )
        .with_context(|| format!("failed to parse flake reference: {}", flake_ref))?;

        debug!(fragment = %fragment, "parsed flake reference");

        // Create lock flags (virtual mode - don't write lock file)
        let mut lock_flags = FlakeLockFlags::new(&self.flake_settings)
            .context("failed to create flake lock flags")?;
        lock_flags
            .set_mode_virtual()
            .context("failed to set virtual lock mode")?;

        // Lock/fetch the flake
        debug!("locking flake");
        let locked_flake = LockedFlake::lock(
            &self.fetchers_settings,
            &self.flake_settings,
            &self.eval_state,
            &lock_flags,
            &flake_reference,
        )
        .with_context(|| format!("failed to lock flake: {}", flake_ref))?;

        // Get the flake outputs
        debug!("getting flake outputs");
        let outputs = locked_flake
            .outputs(&self.flake_settings, &mut self.eval_state)
            .context("failed to get flake outputs")?;

        let outputs = NixValue { inner: outputs };

        // Navigate to the fragment (attribute path) if specified
        if fragment.is_empty() {
            Ok(outputs)
        } else {
            let attr_path: Vec<String> = fragment.split('.').map(String::from).collect();
            debug!(attr_path = ?attr_path, "navigating to attribute");
            self.navigate_attr_path(outputs, &attr_path)
        }
    }

    /// Evaluate a flake reference and build the result.
    ///
    /// Convenience method that combines eval_flake_ref with build_value.
    #[instrument(level = "debug", skip(self), fields(flake_ref = %flake_ref))]
    pub fn eval_and_build_flake_ref(&mut self, flake_ref: &str, base_dir: &Path) -> Result<String> {
        let value = self.eval_flake_ref(flake_ref, base_dir)?;
        self.build_value(&value)
    }

    /// Get attribute names at a path in a flake's outputs without forcing deep evaluation.
    ///
    /// This uses `builtins.getFlake` to properly evaluate the flake (which may copy
    /// to the store for local flakes), then gets attribute names. This avoids issues
    /// with complex flake input patterns that our manual expression generation can't handle.
    #[instrument(level = "debug", skip(self), fields(flake = %flake_path.display(), path = ?attr_path))]
    pub fn eval_flake_attr_names(&mut self, flake_path: &Path, attr_path: &[&str]) -> Result<Vec<String>> {
        let path_str = flake_path
            .to_str()
            .ok_or_else(|| anyhow!("invalid flake path"))?;

        // Build the attribute access chain with `or {}` at each level
        let attr_access = if attr_path.is_empty() {
            "_flake".to_string()
        } else {
            let mut access = "_flake".to_string();
            for attr in attr_path {
                access = format!("({}.{} or {{}})", access, attr);
            }
            access
        };

        // Use builtins.getFlake for reliable flake evaluation
        // This handles complex input patterns that our manual expression can't
        let expr = format!(
            "let _flake = (builtins.getFlake \"{}\"); in builtins.attrNames {}",
            path_str,
            attr_access
        );

        trace!("evaluating attr names expression: {}", expr);
        let value = self.eval_string(&expr, "<trix-attrnames>")?;

        // Parse result as list of strings
        let size = self.require_list_size(&value)?;
        let mut names = Vec::with_capacity(size);
        for i in 0..size {
            let elem = self.require_list_elem(&value, i)?;
            names.push(self.require_string(&elem)?);
        }

        Ok(names)
    }

    /// Get a string value from a Nix value.
    pub fn require_string(&mut self, value: &NixValue) -> Result<String> {
        self.eval_state
            .require_string(&value.inner)
            .map_err(|e| anyhow!("expected string: {}", e))
    }

    /// Coerce a value to a string (handles strings and paths).
    pub fn coerce_to_string(&mut self, value: &NixValue) -> Result<String> {
        let vtype = self.value_type(value)?;
        match vtype {
            ValueType::String => self.require_string(value),
            ValueType::Path => {
                // Use builtins.toString to coerce path to string
                let to_string = self.eval_string("builtins.toString", "<coerce>")?;
                let result = self.apply(to_string, value.clone())?;
                self.require_string(&result)
            }
            _ => Err(anyhow!("expected string or path, got {:?}", vtype)),
        }
    }

    /// Get an integer value from a Nix value.
    pub fn require_int(&mut self, value: &NixValue) -> Result<i64> {
        self.eval_state
            .require_int(&value.inner)
            .map_err(|e| anyhow!("expected int: {}", e))
    }

    /// Check if a value is an attribute set.
    pub fn is_attrs(&mut self, value: &NixValue) -> Result<bool> {
        let vtype = self
            .eval_state
            .value_type(&value.inner)
            .map_err(|e| anyhow!("failed to get value type: {}", e))?;
        Ok(matches!(vtype, ValueType::AttrSet))
    }

    /// Get an attribute from an attribute set.
    pub fn get_attr(&mut self, value: &NixValue, name: &str) -> Result<Option<NixValue>> {
        match self.eval_state.require_attrs_select_opt(&value.inner, name) {
            Ok(Some(v)) => Ok(Some(NixValue { inner: v })),
            Ok(None) => Ok(None),
            Err(e) => Err(anyhow!("failed to get attribute: {}", e)),
        }
    }

    /// Get a boolean value from a Nix value.
    pub fn require_bool(&mut self, value: &NixValue) -> Result<bool> {
        self.eval_state
            .require_bool(&value.inner)
            .map_err(|e| anyhow!("expected bool: {}", e))
    }

    /// Get the size of a list value.
    pub fn require_list_size(&mut self, value: &NixValue) -> Result<usize> {
        let size = self
            .eval_state
            .require_list_size(&value.inner)
            .map_err(|e| anyhow!("expected list: {}", e))?;
        Ok(size as usize)
    }

    /// Get an element from a list value by index.
    pub fn require_list_elem(&mut self, value: &NixValue, index: usize) -> Result<NixValue> {
        let elem = self
            .eval_state
            .require_list_select_idx_strict(&value.inner, index as u32)
            .map_err(|e| anyhow!("failed to get list element: {}", e))?
            .ok_or_else(|| anyhow!("list index out of bounds"))?;
        Ok(NixValue { inner: elem })
    }

    /// Get the type of a value.
    pub fn value_type(&mut self, value: &NixValue) -> Result<ValueType> {
        self.eval_state
            .value_type(&value.inner)
            .map_err(|e| anyhow!("failed to get value type: {}", e))
    }

    /// Get the type name of a value as a string.
    pub fn value_type_name(&mut self, value: &NixValue) -> Result<&'static str> {
        let vtype = self.value_type(value)?;
        Ok(match vtype {
            ValueType::Null => "null",
            ValueType::Bool => "bool",
            ValueType::Int => "int",
            ValueType::Float => "float",
            ValueType::String => "string",
            ValueType::Path => "path",
            ValueType::AttrSet => "set",
            ValueType::List => "list",
            ValueType::Function => "lambda",
            ValueType::External => "external",
            ValueType::Unknown => "unknown",
        })
    }

    /// Get attribute names from an attribute set.
    pub fn get_attr_names(&mut self, value: &NixValue) -> Result<Vec<String>> {
        let names = self
            .eval_state
            .require_attrs_names(&value.inner)
            .map_err(|e| anyhow!("failed to get attribute names: {}", e))?;
        Ok(names)
    }

    /// Convert a Nix value to JSON.
    pub fn value_to_json(&mut self, value: &NixValue) -> Result<serde_json::Value> {
        let vtype = self.value_type(value)?;
        match vtype {
            ValueType::Null => Ok(json!(null)),
            ValueType::Bool => Ok(json!(self.require_bool(value)?)),
            ValueType::Int => Ok(json!(self.require_int(value)?)),
            ValueType::Float => {
                // Float support is limited in the bindings - no require_float method.
                // We use a workaround: coerce to string via Nix's builtins.toString
                // then parse back to f64.
                let coerce_expr = "builtins.toString";
                let to_string_fn = self.eval_string(coerce_expr, "<float-coerce>")?;
                let str_value = self.apply(to_string_fn, value.clone())?;
                let s = self.require_string(&str_value)?;
                let f: f64 = s.parse().unwrap_or(0.0);
                Ok(json!(f))
            }
            ValueType::String => Ok(json!(self.require_string(value)?)),
            ValueType::Path => {
                // Paths are converted to strings in JSON
                let s = self.coerce_to_string(value)?;
                Ok(json!(s))
            }
            ValueType::AttrSet => {
                let names = self.get_attr_names(value)?;
                let mut map = serde_json::Map::new();
                for name in names {
                    if let Some(attr_value) = self.get_attr(value, &name)? {
                        let json_value = self.value_to_json(&attr_value)?;
                        map.insert(name, json_value);
                    }
                }
                Ok(serde_json::Value::Object(map))
            }
            ValueType::List => {
                let size = self.require_list_size(value)?;
                let mut arr = Vec::with_capacity(size);
                for i in 0..size {
                    let elem = self.require_list_elem(value, i)?;
                    arr.push(self.value_to_json(&elem)?);
                }
                Ok(serde_json::Value::Array(arr))
            }
            ValueType::Function => Err(anyhow!("cannot convert function to JSON")),
            ValueType::External => Err(anyhow!("cannot convert external to JSON")),
            ValueType::Unknown => Err(anyhow!("cannot convert unknown value to JSON")),
        }
    }

    /// Print a Nix value in Nix format.
    pub fn value_to_nix_string(&mut self, value: &NixValue) -> Result<String> {
        let vtype = self.value_type(value)?;
        match vtype {
            ValueType::Null => Ok("null".to_string()),
            ValueType::Bool => {
                let b = self.require_bool(value)?;
                Ok(if b { "true" } else { "false" }.to_string())
            }
            ValueType::Int => Ok(self.require_int(value)?.to_string()),
            ValueType::Float => {
                // Float support is limited in the bindings - no require_float method.
                // Coerce to string via builtins.toString.
                let coerce_expr = "builtins.toString";
                let to_string_fn = self.eval_string(coerce_expr, "<float-coerce>")?;
                let str_value = self.apply(to_string_fn, value.clone())?;
                self.require_string(&str_value)
            }
            ValueType::String => {
                let s = self.require_string(value)?;
                Ok(format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\"")))
            }
            ValueType::Path => {
                let s = self.coerce_to_string(value)?;
                Ok(s)
            }
            ValueType::AttrSet => {
                // Check if this is a derivation (has type = "derivation" and drvPath)
                if let Some(type_attr) = self.get_attr(value, "type")? {
                    if self.value_type(&type_attr)? == ValueType::String {
                        if let Ok(type_str) = self.require_string(&type_attr) {
                            if type_str == "derivation" {
                                // Format as «derivation /nix/store/...-name.drv»
                                if let Some(drv_path_attr) = self.get_attr(value, "drvPath")? {
                                    if let Ok(drv_path) = self.require_string(&drv_path_attr) {
                                        return Ok(format!("«derivation {}»", drv_path));
                                    }
                                }
                                return Ok("«derivation»".to_string());
                            }
                        }
                    }
                }

                let names = self.get_attr_names(value)?;
                if names.is_empty() {
                    return Ok("{ }".to_string());
                }
                let mut parts = Vec::new();
                for name in &names {
                    if let Some(attr_value) = self.get_attr(value, name)? {
                        let value_str = self.value_to_nix_string(&attr_value)?;
                        // Quote attribute names if they contain special chars
                        let name_str = if name.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-') {
                            name.clone()
                        } else {
                            format!("\"{}\"", name)
                        };
                        parts.push(format!("{} = {};", name_str, value_str));
                    }
                }
                Ok(format!("{{ {} }}", parts.join(" ")))
            }
            ValueType::List => {
                let size = self.require_list_size(value)?;
                if size == 0 {
                    return Ok("[ ]".to_string());
                }
                let mut parts = Vec::new();
                for i in 0..size {
                    let elem = self.require_list_elem(value, i)?;
                    parts.push(self.value_to_nix_string(&elem)?);
                }
                Ok(format!("[ {} ]", parts.join(" ")))
            }
            ValueType::Function => Ok("«lambda»".to_string()),
            ValueType::External => Ok("«external»".to_string()),
            ValueType::Unknown => Ok("«unknown»".to_string()),
        }
    }

    /// Evaluate a Nix file.
    pub fn eval_file(&mut self, path: &Path) -> Result<NixValue> {
        let path_str = path
            .to_str()
            .ok_or_else(|| anyhow!("invalid file path"))?;
        let expr = format!("import {}", path_str);
        self.eval_string(&expr, path_str)
    }

    /// Apply a function to a value.
    pub fn apply(&mut self, func: NixValue, arg: NixValue) -> Result<NixValue> {
        let result = self
            .eval_state
            .call(func.inner, arg.inner)
            .map_err(|e| anyhow!("function application failed: {}", e))?;
        Ok(NixValue { inner: result })
    }

    // =========================================================================
    // Native Build Methods
    // =========================================================================

    /// Check if a value is a derivation (has type = "derivation").
    pub fn is_derivation(&mut self, value: &NixValue) -> Result<bool> {
        if !self.is_attrs(value)? {
            return Ok(false);
        }
        match self.get_attr(value, "type")? {
            Some(type_attr) => {
                if self.value_type(&type_attr)? == ValueType::String {
                    Ok(self.require_string(&type_attr)? == "derivation")
                } else {
                    Ok(false)
                }
            }
            None => Ok(false),
        }
    }

    /// Get the .drv path from a derivation value.
    ///
    /// Returns the path string (e.g., "/nix/store/xxx.drv").
    #[instrument(level = "debug", skip(self, value))]
    pub fn get_drv_path(&mut self, value: &NixValue) -> Result<String> {
        let drv_path_attr = self
            .get_attr(value, "drvPath")?
            .ok_or_else(|| anyhow!("value is not a derivation (no drvPath attribute)"))?;
        self.require_string(&drv_path_attr)
    }

    /// Parse a store path string into a StorePath object.
    #[instrument(level = "trace", skip(self), fields(path = %path))]
    pub fn parse_store_path(&mut self, path: &str) -> Result<StorePath> {
        self.store
            .parse_store_path(path)
            .map_err(|e| anyhow!("failed to parse store path '{}': {}", path, e))
    }

    /// Build a derivation value and return the default output path.
    ///
    /// Uses `realise_string` on the derivation's `outPath` attribute, which
    /// builds the derivation through the string context mechanism.
    #[instrument(level = "debug", skip(self, value))]
    pub fn build_value(&mut self, value: &NixValue) -> Result<String> {
        // Get the outPath attribute - this is a string with derivation context
        let out_path_attr = self
            .get_attr(value, "outPath")?
            .ok_or_else(|| anyhow!("value is not a derivation (no outPath attribute)"))?;

        debug!("building derivation via realise_string");

        // realise_string builds any derivations in the string's context
        let realised = self
            .eval_state
            .realise_string(&out_path_attr.inner, false)
            .map_err(|e| anyhow!("build failed: {}", e))?;

        let output_path = realised.s;
        debug!(output = %output_path, "build completed");
        Ok(output_path)
    }

    /// Build a derivation and return all output paths.
    ///
    /// This builds the derivation and returns a map of output names to paths.
    /// For derivations with multiple outputs, use this instead of `build_value`.
    #[instrument(level = "debug", skip(self, value))]
    pub fn build_value_outputs(&mut self, value: &NixValue) -> Result<BTreeMap<String, String>> {
        // First, get the list of output names
        let outputs_attr = self
            .get_attr(value, "outputs")?
            .ok_or_else(|| anyhow!("value is not a derivation (no outputs attribute)"))?;

        let output_count = self.require_list_size(&outputs_attr)?;
        let mut results = BTreeMap::new();

        for i in 0..output_count {
            let output_name_value = self.require_list_elem(&outputs_attr, i)?;
            let output_name = self.require_string(&output_name_value)?;

            // Get the output path attribute (e.g., value.out, value.dev)
            let output_path_attr = self
                .get_attr(value, &output_name)?
                .ok_or_else(|| anyhow!("derivation missing output '{}'", output_name))?;

            // realise_string builds the derivation through context
            let realised = self
                .eval_state
                .realise_string(&output_path_attr.inner, false)
                .map_err(|e| anyhow!("build failed for output '{}': {}", output_name, e))?;

            results.insert(output_name, realised.s);
        }

        debug!(outputs = ?results.keys().collect::<Vec<_>>(), "build completed");
        Ok(results)
    }

    /// Evaluate a flake attribute and build it, returning the default output path.
    ///
    /// This combines eval_flake_attr and build_value into a single operation.
    #[instrument(level = "debug", skip(self), fields(flake = %flake_path.display(), attr = ?attr_path))]
    pub fn eval_and_build_flake_attr(
        &mut self,
        flake_path: &Path,
        attr_path: &[String],
    ) -> Result<String> {
        let value = self.eval_flake_attr(flake_path, attr_path)?;
        self.build_value(&value)
    }

    /// Get the default output path from a multi-output build result.
    ///
    /// Returns the "out" output path, or the first output if "out" doesn't exist.
    pub fn default_output(outputs: &BTreeMap<String, String>) -> Result<&str> {
        outputs
            .get("out")
            .or_else(|| outputs.values().next())
            .map(|s| s.as_str())
            .ok_or_else(|| anyhow!("derivation produced no outputs"))
    }

    /// Get the main program name for a derivation.
    ///
    /// Tries (in order):
    /// 1. meta.mainProgram attribute
    /// 2. pname attribute
    /// 3. name attribute (with version suffix stripped)
    /// 4. Provided fallback name
    #[instrument(level = "debug", skip(self, value))]
    pub fn get_main_program(&mut self, value: &NixValue, fallback: &str) -> Result<String> {
        // Try meta.mainProgram first
        if let Some(meta) = self.get_attr(value, "meta")? {
            if let Some(main_program) = self.get_attr(&meta, "mainProgram")? {
                if self.value_type(&main_program)? == ValueType::String {
                    if let Ok(s) = self.require_string(&main_program) {
                        debug!(main_program = %s, "found meta.mainProgram");
                        return Ok(s);
                    }
                }
            }
        }

        // Try pname
        if let Some(pname_attr) = self.get_attr(value, "pname")? {
            if self.value_type(&pname_attr)? == ValueType::String {
                if let Ok(pname) = self.require_string(&pname_attr) {
                    debug!(pname = %pname, "using pname as mainProgram");
                    return Ok(pname);
                }
            }
        }

        // Try name (strip version suffix)
        if let Some(name_attr) = self.get_attr(value, "name")? {
            if self.value_type(&name_attr)? == ValueType::String {
                if let Ok(name) = self.require_string(&name_attr) {
                    // Strip version suffix (last -X.Y.Z part)
                    if let Some(pos) = name.rfind('-') {
                        let suffix = &name[pos + 1..];
                        if suffix
                            .chars()
                            .next()
                            .map(|c| c.is_ascii_digit())
                            .unwrap_or(false)
                        {
                            let program_name = &name[..pos];
                            debug!(name = %name, program = %program_name, "using name (stripped) as mainProgram");
                            return Ok(program_name.to_string());
                        }
                    }
                    debug!(name = %name, "using name as mainProgram");
                    return Ok(name);
                }
            }
        }

        debug!(fallback = %fallback, "using fallback as mainProgram");
        Ok(fallback.to_string())
    }
}

/// Generate a Nix expression to evaluate a flake without copying to store.
///
/// This is the key to trix - we generate Nix code that:
/// 1. Fetches remote inputs using builtins.fetchTarball/fetchGit (cached in store, fine)
/// 2. Imports the local flake.nix directly (NOT via getFlake)
/// 3. Constructs the inputs attrset from flake.lock data
/// 4. Calls flake.outputs with the constructed inputs
///
/// The `input_overrides` parameter allows overriding specific inputs with local paths,
/// which are imported directly without copying to the store (just like the main flake).
///
/// This function is public because it's also used by Evaluator::eval_flake_attr
/// to generate expressions for internal evaluation.
pub fn generate_flake_eval_expr(
    flake_dir: &str,
    lock: &FlakeLock,
    attr_path: &[String],
    input_overrides: &HashMap<String, String>,
) -> Result<String> {
    // Get root node's inputs
    let root_node = lock.nodes.get(&lock.root);
    let root_inputs: HashMap<String, InputRef> = root_node
        .map(|n| n.inputs.clone())
        .unwrap_or_default();

    // Build topologically sorted list of nodes (dependencies first)
    let sorted_nodes = topological_sort_nodes(lock)?;

    // Generate fetch expressions for each node
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
            let override_expr = generate_override_input_expr(
                node_name,
                &resolved_path,
                node.flake,
            )?;
            let_bindings.push(override_expr);
            continue;
        }

        // Generate the source fetch expression (normal path)
        let src_binding = if let Some(ref locked) = node.locked {
            let fetch_expr = generate_fetch_expr(locked, flake_dir);
            format!("_src_{} = {};", sanitize_name(node_name), fetch_expr)
        } else {
            continue; // No locked ref, skip (shouldn't happen for non-root)
        };

        let_bindings.push(src_binding);

        // If it's a flake, generate the input building expression
        if node.flake {
            let input_expr = generate_input_build_expr(node_name, node, lock, flake_dir)?;
            let_bindings.push(format!("{} = {};", sanitize_name(node_name), input_expr));
        } else {
            // Non-flake input - just use the source
            let_bindings.push(format!(
                "{name} = {{ outPath = _src_{name}; }};",
                name = sanitize_name(node_name)
            ));
        }
    }

    // Build the root inputs attrset
    // Use quoted attribute names to preserve hyphens (e.g., "flake-utils" = flake_utils)
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
    // Must use original input names (with hyphens) as attribute names, mapped to sanitized variables
    let mut output_args = vec!["self = self".to_string()];
    for (input_name, sanitized) in &resolved_root_inputs {
        // Quote the attribute name to preserve hyphens (e.g., "flake-utils" = flake_utils)
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
  flakeDirPath = {flake_dir};

  # Minimal self for nested inputs that follow root (defined before fetched sources)
  # This is needed when a nested flake's input uses "follows": [] (empty follows = root self)
  _rootSelf = {{
    outPath = flakeDirPath;
    _type = "flake";
    {git_attrs}
  }};

  # Fetched sources and built inputs
  {let_bindings}

  # Self input (the local flake) with full inputs
  self = _rootSelf // {{
    inputs = {{ {input_attrs} }};
  }};

  # Import and evaluate the flake
  flake = import (flakeDirPath + "/flake.nix");
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

/// Generate a Nix fetch expression for a locked reference.
fn generate_fetch_expr(locked: &LockedRef, flake_dir: &str) -> String {
    match locked {
        LockedRef::GitHub { owner, repo, rev, nar_hash, .. } => {
            let hash_arg = nar_hash.as_ref()
                .map(|h| format!(" sha256 = \"{}\";", h))
                .unwrap_or_default();
            format!(
                r#"builtins.fetchTarball {{ url = "https://github.com/{}/{}/archive/{}.tar.gz";{} }}"#,
                owner, repo, rev, hash_arg
            )
        }
        LockedRef::GitLab { owner, repo, rev, nar_hash, .. } => {
            let hash_arg = nar_hash.as_ref()
                .map(|h| format!(" sha256 = \"{}\";", h))
                .unwrap_or_default();
            format!(
                r#"builtins.fetchTarball {{ url = "https://gitlab.com/{}/{}/-/archive/{}/{}-{}.tar.gz";{} }}"#,
                owner, repo, rev, repo, rev, hash_arg
            )
        }
        LockedRef::Sourcehut { owner, repo, rev, nar_hash, .. } => {
            let hash_arg = nar_hash.as_ref()
                .map(|h| format!(" sha256 = \"{}\";", h))
                .unwrap_or_default();
            format!(
                r#"builtins.fetchTarball {{ url = "https://git.sr.ht/~{}/{}/archive/{}.tar.gz";{} }}"#,
                owner, repo, rev, hash_arg
            )
        }
        LockedRef::Git { url, rev, nar_hash, git_ref } => {
            let ref_arg = git_ref.as_ref()
                .map(|r| format!(" ref = \"{}\";", r))
                .unwrap_or_default();
            let hash_arg = nar_hash.as_ref()
                .map(|h| format!(" narHash = \"{}\";", h))
                .unwrap_or_default();
            format!(
                r#"builtins.fetchGit {{ url = "{}"; rev = "{}";{}{} }}"#,
                url, rev, ref_arg, hash_arg
            )
        }
        LockedRef::Path { path, .. } => {
            if path.starts_with('/') {
                format!("/. + \"{}\"", path)
            } else {
                // Need a "/" separator for relative paths like "../foo"
                format!("{} + \"/{}\"", flake_dir, path)
            }
        }
        LockedRef::Tarball { url, nar_hash, .. } => {
            let hash_arg = nar_hash.as_ref()
                .map(|h| format!(" sha256 = \"{}\";", h))
                .unwrap_or_default();
            format!(r#"builtins.fetchTarball {{ url = "{}";{} }}"#, url, hash_arg)
        }
        LockedRef::Indirect { id, .. } => {
            // Indirect refs should be resolved before locking
            format!(r#"throw "trix: unresolved indirect input '{}' - run trix lock first""#, id)
        }
    }
}

/// Generate expression to build an input from its source.
fn generate_input_build_expr(
    node_name: &str,
    node: &crate::lock::LockNode,
    lock: &FlakeLock,
    _flake_dir: &str,
) -> Result<String> {
    let src_name = format!("_src_{}", sanitize_name(node_name));

    // Build this input's inputs
    let mut input_exprs = Vec::new();
    for (input_name, input_ref) in &node.inputs {
        let resolved = match input_ref {
            InputRef::Direct(name) => sanitize_name(name),
            InputRef::Follows(path) => {
                match resolve_follows_to_name(path, lock)? {
                    FollowsResolution::Node(name) => name,
                    // Empty follows at nested level means "follows root's self"
                    // For nested inputs, this would be the root flake's self, but since
                    // we're building this input before self is defined, we use _rootSelf
                    FollowsResolution::Self_ => "_rootSelf".to_string(),
                }
            }
        };
        // Quote the attribute name to preserve hyphens (e.g., "nixpkgs-lib" = nixpkgs)
        input_exprs.push(format!("\"{}\" = {};", input_name, resolved));
    }

    let inputs_str = input_exprs.join(" ");

    Ok(format!(
        r#"let
    _flake = import ({src} + "/flake.nix");
    _inputs = {{ {inputs} }};
    _self = {{ outPath = {src}; inputs = _inputs; _type = "flake"; }};
    _outputs = _flake.outputs (_inputs // {{ self = _self // _outputs; }});
  in _outputs // {{ outPath = {src}; inputs = _inputs; outputs = _outputs; _type = "flake"; }}"#,
        src = src_name,
        inputs = inputs_str,
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
                        // For follows, we need to resolve to find the actual node
                        if let Some(first) = path.first() {
                            first.clone()
                        } else {
                            continue;
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
                InputRef::Follows(_) => continue, // Root-level follows handled separately
            };
            visit(node_name, lock, &mut sorted, &mut visited, &mut in_progress)?;
        }
    }

    Ok(sorted)
}

/// Resolve a follows path to a node name.
/// Returns a special marker for empty follows (which means "follows self/root").
fn resolve_follows_to_name(path: &[String], lock: &FlakeLock) -> Result<FollowsResolution> {
    // Empty follows path means "follows self/root" - the input is the root flake itself
    if path.is_empty() {
        return Ok(FollowsResolution::Self_);
    }

    // Simple follows resolution - just get the final target
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
    /// Points to a named node (the sanitized variable name)
    Node(String),
    /// Points to self/root (empty follows path)
    Self_,
}

/// Sanitize a name for use as a Nix identifier.
fn sanitize_name(name: &str) -> String {
    // Replace hyphens and other special chars with underscores for variable names
    // But keep original for attribute access
    name.replace('-', "_")
}

/// Get git metadata for a directory without copying to the nix store.
/// Returns Nix attribute syntax for rev, shortRev, lastModified, lastModifiedDate.
fn get_git_metadata(flake_dir: &str) -> String {
    let repo = match git2::Repository::discover(flake_dir) {
        Ok(r) => r,
        Err(_) => {
            // Not a git repo - return minimal attrs
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

    // Format lastModifiedDate as YYYYMMDD
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
        // ~user/path - not supported for simplicity
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
///
/// This handles the case where --override-input specifies a local path.
/// We import the flake.nix directly and read its flake.lock to construct inputs.
fn generate_override_input_expr(
    node_name: &str,
    override_path: &str,
    _is_flake_hint: bool,
) -> Result<String> {
    let sanitized = sanitize_name(node_name);

    // Determine if the override path is a flake by checking for flake.nix
    // This is more reliable than using the original lock's flake flag
    let flake_nix_path = Path::new(override_path).join("flake.nix");
    let is_flake = flake_nix_path.exists();

    if !is_flake {
        // Non-flake override - just use direct path
        return Ok(format!(
            "{name} = {{ outPath = {path}; }};",
            name = sanitized,
            path = override_path
        ));
    }

    // For flake overrides, we need to construct the full input
    // Read the override's flake.lock if it exists
    let lock_path = Path::new(override_path).join("flake.lock");

    let (inputs_expr, nested_bindings) = if lock_path.exists() {
        // Parse the override's lock file
        let lock_content = std::fs::read_to_string(&lock_path)
            .with_context(|| format!("failed to read {}", lock_path.display()))?;
        let override_lock: FlakeLock = serde_json::from_str(&lock_content)
            .with_context(|| format!("failed to parse {}", lock_path.display()))?;

        // Generate expressions for the override's inputs
        generate_override_inputs(&override_lock, override_path)?
    } else {
        // No lock file - the override has no inputs
        ("{ }".to_string(), String::new())
    };

    // Get git metadata for the override path
    let git_attrs = get_git_metadata(override_path);

    Ok(format!(
        r#"# Overridden input: {node_name} -> {override_path}
  {nested_bindings}{name} = let
    _override_path = {path};
    _flake = import (_override_path + "/flake.nix");
    _inputs = {inputs};
    _self = {{ outPath = _override_path; inputs = _inputs; _type = "flake"; {git_attrs} }};
    _outputs = _flake.outputs (_inputs // {{ self = _self // _outputs; }});
  in _outputs // {{ outPath = _override_path; inputs = _inputs; outputs = _outputs; _type = "flake"; }};"#,
        node_name = node_name,
        override_path = override_path,
        nested_bindings = nested_bindings,
        name = sanitized,
        path = override_path,
        inputs = inputs_expr,
        git_attrs = git_attrs,
    ))
}

/// Generate input expressions for an override's dependencies.
/// Returns (inputs_attrset_expr, nested_let_bindings).
fn generate_override_inputs(lock: &FlakeLock, override_path: &str) -> Result<(String, String)> {
    let root_node = match lock.nodes.get(&lock.root) {
        Some(node) => node,
        None => return Ok(("{ }".to_string(), String::new())),
    };

    if root_node.inputs.is_empty() {
        return Ok(("{ }".to_string(), String::new()));
    }

    // Sort nodes topologically
    let sorted_nodes = topological_sort_nodes(lock)?;

    let mut let_bindings = Vec::new();
    let mut input_attrs = Vec::new();

    // Generate fetch/build expressions for each input
    for node_name in &sorted_nodes {
        if node_name == &lock.root {
            continue;
        }

        let node = lock.nodes.get(node_name)
            .ok_or_else(|| anyhow!("node '{}' not found in override lock", node_name))?;

        // Generate fetch expression
        if let Some(ref locked) = node.locked {
            let fetch_expr = generate_fetch_expr(locked, override_path);
            let_bindings.push(format!(
                "_override_src_{} = {};",
                sanitize_name(node_name),
                fetch_expr
            ));

            // Build the input
            if node.flake {
                let src_name = format!("_override_src_{}", sanitize_name(node_name));
                let nested_inputs = generate_nested_input_refs(&node.inputs, lock, "_override_")?;
                let_bindings.push(format!(
                    r#"_override_{name} = let
      _flake = import ({src} + "/flake.nix");
      _inputs = {{ {nested_inputs} }};
      _self = {{ outPath = {src}; inputs = _inputs; _type = "flake"; }};
      _outputs = _flake.outputs (_inputs // {{ self = _self // _outputs; }});
    in _outputs // {{ outPath = {src}; inputs = _inputs; outputs = _outputs; _type = "flake"; }};"#,
                    name = sanitize_name(node_name),
                    src = src_name,
                    nested_inputs = nested_inputs,
                ));
            } else {
                let_bindings.push(format!(
                    "_override_{name} = {{ outPath = _override_src_{name}; }};",
                    name = sanitize_name(node_name)
                ));
            }
        }
    }

    // Build root inputs attrset
    for (input_name, input_ref) in &root_node.inputs {
        let resolved = match input_ref {
            InputRef::Direct(name) => format!("_override_{}", sanitize_name(name)),
            InputRef::Follows(path) => {
                if path.is_empty() {
                    "_self".to_string() // Points to the override's self
                } else {
                    match resolve_follows_to_name(path, lock)? {
                        FollowsResolution::Node(name) => format!("_override_{}", name),
                        FollowsResolution::Self_ => "_self".to_string(),
                    }
                }
            }
        };
        input_attrs.push(format!("\"{}\" = {};", input_name, resolved));
    }

    let bindings_str = if let_bindings.is_empty() {
        String::new()
    } else {
        format!("{}\n  ", let_bindings.join("\n  "))
    };

    Ok((format!("{{ {} }}", input_attrs.join(" ")), bindings_str))
}

/// Generate input references for nested inputs (within an override's input).
fn generate_nested_input_refs(
    inputs: &HashMap<String, InputRef>,
    lock: &FlakeLock,
    prefix: &str,
) -> Result<String> {
    let mut refs = Vec::new();
    for (input_name, input_ref) in inputs {
        let resolved = match input_ref {
            InputRef::Direct(name) => format!("{}{}", prefix, sanitize_name(name)),
            InputRef::Follows(path) => {
                if path.is_empty() {
                    "_self".to_string()
                } else {
                    match resolve_follows_to_name(path, lock)? {
                        FollowsResolution::Node(name) => format!("{}{}", prefix, name),
                        FollowsResolution::Self_ => "_self".to_string(),
                    }
                }
            }
        };
        refs.push(format!("\"{}\" = {};", input_name, resolved));
    }
    Ok(refs.join(" "))
}

/// A Nix value wrapper.
#[derive(Clone)]
pub struct NixValue {
    inner: nix_bindings_expr::value::Value,
}

impl NixValue {
    /// Get the type name of this value.
    pub fn type_name(&self) -> &'static str {
        // TODO: implement proper type detection
        "value"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eval_simple_string() {
        let mut eval = Evaluator::new().unwrap();
        let value = eval.eval_string(r#""hello world""#, "<test>").unwrap();
        let s = eval.require_string(&value).unwrap();
        assert_eq!(s, "hello world");
    }

    #[test]
    fn eval_simple_int() {
        let mut eval = Evaluator::new().unwrap();
        let value = eval.eval_string("42", "<test>").unwrap();
        let n = eval.require_int(&value).unwrap();
        assert_eq!(n, 42);
    }

    #[test]
    fn eval_arithmetic() {
        let mut eval = Evaluator::new().unwrap();
        let value = eval.eval_string("1 + 2 + 3", "<test>").unwrap();
        let n = eval.require_int(&value).unwrap();
        assert_eq!(n, 6);
    }

    #[test]
    fn eval_let_binding() {
        let mut eval = Evaluator::new().unwrap();
        let value = eval
            .eval_string("let x = 10; y = 20; in x + y", "<test>")
            .unwrap();
        let n = eval.require_int(&value).unwrap();
        assert_eq!(n, 30);
    }

    #[test]
    fn eval_string_interpolation() {
        let mut eval = Evaluator::new().unwrap();
        let value = eval
            .eval_string(r#"let name = "world"; in "hello ${name}""#, "<test>")
            .unwrap();
        let s = eval.require_string(&value).unwrap();
        assert_eq!(s, "hello world");
    }

    #[test]
    fn eval_attrset() {
        let mut eval = Evaluator::new().unwrap();
        let value = eval.eval_string("{ foo = 1; bar = 2; }", "<test>").unwrap();
        assert!(eval.is_attrs(&value).unwrap());
    }

    #[test]
    fn eval_get_attr() {
        let mut eval = Evaluator::new().unwrap();
        let value = eval
            .eval_string("{ foo = 42; bar = 2; }", "<test>")
            .unwrap();
        let foo = eval.get_attr(&value, "foo").unwrap().unwrap();
        let n = eval.require_int(&foo).unwrap();
        assert_eq!(n, 42);
    }

    #[test]
    fn eval_nested_attrs() {
        let mut eval = Evaluator::new().unwrap();
        let value = eval
            .eval_string("{ a.b.c = 123; }", "<test>")
            .unwrap();
        let a = eval.get_attr(&value, "a").unwrap().unwrap();
        let b = eval.get_attr(&a, "b").unwrap().unwrap();
        let c = eval.get_attr(&b, "c").unwrap().unwrap();
        let n = eval.require_int(&c).unwrap();
        assert_eq!(n, 123);
    }

    #[test]
    fn eval_simple_derivation_drv_path() {
        // Test if we can evaluate a simple derivation's drvPath
        // This derivation does NOT use any local paths
        let mut eval = Evaluator::new().unwrap();
        let value = eval
            .eval_string(
                r#"
                (derivation {
                    name = "simple-test";
                    system = builtins.currentSystem;
                    builder = "/bin/sh";
                    args = ["-c" "echo hello > $out"];
                }).drvPath
                "#,
                "<test>",
            )
            .unwrap();
        let drv_path = eval.require_string(&value).unwrap();
        assert!(drv_path.starts_with("/nix/store/"));
        assert!(drv_path.ends_with(".drv"));
    }

    #[test]
    fn eval_path_to_string() {
        // Test if we can evaluate a path (not adding to store)
        let mut eval = Evaluator::new().unwrap();
        let value = eval
            .eval_string("builtins.toString /tmp", "<test>")
            .unwrap();
        let s = eval.require_string(&value).unwrap();
        assert_eq!(s, "/tmp");
    }

    // NOTE: Tests for path handling in derivations
    //
    // Note on path coercion:
    //
    // Path coercion (automatically adding local paths to the store when referenced
    // in derivations) now works correctly with nix-bindings. This was fixed by
    // ensuring `eval_state_builder_load()` is called during EvalState initialization,
    // which sets `readOnlyMode = false` and enables proper path coercion.
    //
    // Previously this was a limitation, but the fix has been applied upstream in
    // nix-bindings-rust, so all trix commands now use native evaluation.

    #[test]
    fn eval_builtins_path_works() {
        // This test confirms that builtins.path CAN add files to the store
        let mut eval = Evaluator::new().unwrap();

        let test_file = std::env::temp_dir().join("nix-builtin-path-test.txt");
        std::fs::write(&test_file, "test content for builtins.path").unwrap();

        let expr = format!(
            r#"builtins.path {{ path = {}; name = "test-file"; }}"#,
            test_file.display()
        );

        let result = eval.eval_string(&expr, "<test>");
        std::fs::remove_file(&test_file).ok();

        let value = result.expect("builtins.path should work");
        let s = eval.require_string(&value).unwrap();
        assert!(s.starts_with("/nix/store/"), "Should be a store path");
    }

    #[test]
    fn eval_derivation_with_builtins_path_works() {
        // This test confirms that wrapping paths with builtins.path works in derivations
        let mut eval = Evaluator::new().unwrap();

        let test_file = std::env::temp_dir().join("nix-drv-builtins-path-test.txt");
        std::fs::write(&test_file, "test content").unwrap();

        let expr = format!(
            r#"
            let
              srcPath = builtins.path {{ path = {}; name = "test-src"; }};
            in (derivation {{
                name = "path-test-with-builtins";
                system = builtins.currentSystem;
                builder = "/bin/sh";
                args = ["-c" "cat $src > $out"];
                src = srcPath;
            }}).drvPath
            "#,
            test_file.display()
        );

        let result = eval.eval_string(&expr, "<test>");
        std::fs::remove_file(&test_file).ok();

        let value = result.expect("derivation with builtins.path should work");
        let drv_path = eval.require_string(&value).unwrap();
        assert!(drv_path.starts_with("/nix/store/"), "drvPath should be in /nix/store");
        assert!(drv_path.ends_with(".drv"), "drvPath should end with .drv");
    }

    #[test]
    fn native_build_simple_derivation() {
        // Test the native build_value method using realise_string
        let mut eval = Evaluator::new().unwrap();

        // A simple derivation that just echoes to $out
        let expr = r#"
            derivation {
                name = "native-build-test";
                system = builtins.currentSystem;
                builder = "/bin/sh";
                args = ["-c" "echo 'hello from native build' > $out"];
            }
        "#;

        let value = eval.eval_string(expr, "<test>").unwrap();

        // Verify it's a derivation
        assert!(eval.is_derivation(&value).unwrap());

        // Get the drv path
        let drv_path = eval.get_drv_path(&value).unwrap();
        assert!(drv_path.ends_with(".drv"));

        // Build it using the native build_value method
        let output_path = eval.build_value(&value).unwrap();

        assert!(output_path.starts_with("/nix/store/"));
        assert!(output_path.contains("native-build-test"));

        // Verify the output file exists and has expected content
        let content = std::fs::read_to_string(&output_path).unwrap();
        assert!(content.contains("hello from native build"));
    }
}
