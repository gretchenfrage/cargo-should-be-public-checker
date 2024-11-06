
use crate::error::*;
use std::{
    fs::{File, canonicalize},
    io::BufReader,
    env::temp_dir,
    path::{self, Path, PathBuf},
};


pub fn build_rustdoc_json(
    manifest_path: impl AsRef<Path>,
    package: &str,
) -> Result<rustdoc_types::Crate> {
    let package = package.replace("_", "-");
    build_rustdoc_json_inner(manifest_path.as_ref(), &package)
        .or_else(|e| {
            // TODO: utterly disgusting
            let package_underscores = package.replace("-", "_");
            if package != package_underscores {
                build_rustdoc_json_inner(manifest_path.as_ref(), &package_underscores)
            } else {
                Err(e)
            }
        })
}

fn build_rustdoc_json_inner(
    manifest_path: impl AsRef<Path>,
    package: &str,
) -> Result<rustdoc_types::Crate> {
    let json_path = rustdoc_json::Builder::default()
        .toolchain("nightly")
        .target_dir(&target_dir(manifest_path.as_ref()))
        .document_private_items(true) // TODO: it is unfortunate we have to do this for now(?)
        .package(package)
        .manifest_path(manifest_path)
        .build()
        .wrap_err("Failed to build rustdoc JSON")?;
    let file = File::open(json_path).wrap_err("Failed to open rustdoc JSON file")?;
    serde_json::from_reader::<_, rustdoc_types::Crate>(BufReader::new(file))
        .wrap_err("Failed to deserialize rustdoc JSON output")
    // TODO: assert rustdoc_types::FORMAT_VERSION match
}

fn target_dir(manifest_path: &Path) -> PathBuf {
    let mut target_dir = temp_dir();
    target_dir.push("should-be-public-checker-targets");
    target_dir.push(canonicalize(manifest_path)
        .unwrap_or_else(|_| manifest_path.into())
        .components()
        .filter_map(|component| match component {
            path::Component::Normal(os_str) => Some(os_str),
            _ => None,
        })
        .map(|os_str| os_str.to_string_lossy())
        .fold(String::new(), |mut buf, part| {
            if !buf.is_empty() {
                buf.push('_');
            }
            buf.push_str(&part);
            buf
        }));
    target_dir
}
