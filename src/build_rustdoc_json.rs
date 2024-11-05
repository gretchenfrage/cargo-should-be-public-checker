
use crate::error::*;
use std::{
    fs::{File, canonicalize},
    io::BufReader,
    env::temp_dir,
    path::{self, Path, PathBuf},
};


pub fn build_rustdoc_json(
    manifest_path: impl AsRef<Path>,
    package: Option<&str>,
) -> Result<rustdoc_types::Crate> {
    let mut builder = rustdoc_json::Builder::default()
        .toolchain("nightly")
        .target_dir(&target_dir(manifest_path.as_ref(), package))
        .manifest_path(manifest_path);
    if let Some(package) = package {
        builder = builder.package(package);
    }
    let json_path = builder.build().wrap_err("Failed to build rustdoc JSON")?;
    let file = File::open(json_path).wrap_err("Failed to open rustdoc JSON file")?;
    serde_json::from_reader::<_, rustdoc_types::Crate>(BufReader::new(file))
        .wrap_err("Failed to deserialize rustdoc JSON output")
    // TODO: assert rustdoc_types::FORMAT_VERSION match
}

fn target_dir(manifest_path: &Path, package: Option<&str>) -> PathBuf {
    let mut target_dir = temp_dir();
    target_dir.push("should-be-public-checker-targets");
    target_dir.push(canonicalize(manifest_path)
        .expect("failed to canonicalize manifest path")
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
    if let Some(package) = package {
        target_dir.push(package);
    }
    target_dir
}
