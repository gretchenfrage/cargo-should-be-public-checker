
use crate::error::*;
use std::{
    path::Path,
    process::Command,
};
use serde_json::Value;


/// Assuming the given manifest path has a default package, get that package name such that running
/// cargo commands with `--package ${PACKAGE_NAME}` would not change their behavior.
pub fn default_package_name(manifest_path: impl AsRef<Path>) -> Result<String> {
    let output = Command::new("cargo")
        .arg("metadata")
        .arg("--no-deps")
        .arg("--format-version=1")
        .arg("--manifest-path")
        .arg(manifest_path.as_ref())
        .output()?;

    ensure!(output.status.success(), "Failed to run cargo metadata");

    serde_json::from_slice::<Value>(&output.stdout)
        .wrap_err("Failed to parse output of cargo metadata")?
        .get("packages")
        .and_then(|value| value.get(0))
        .and_then(|value| value.get("name"))
        .and_then(|value| value.as_str())
        .map(String::from)
        .ok_or_eyre("Failed to extract package name from output of cargo metadata")
}
