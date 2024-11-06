
use crate::{
    error::*,
    build_rustdoc_json::build_rustdoc_json,
    cargo_metadata::default_package_name,
};
use std::path::PathBuf;
use clap::Parser;


const CARGO_TOML: &'static str = "Cargo.toml";

#[derive(Parser, Debug)]
pub struct CliArgs {
    #[arg(default_value = ".")]
    pub path: PathBuf,
    #[arg(short, long)]
    pub package: Option<String>,
}

impl CliArgs {
    pub fn root_package(&self) -> Result<String> {
        self.package.clone()
            .map(Ok)
            .unwrap_or_else(|| default_package_name(self.path.join(CARGO_TOML)))
    }

    pub fn build_rustdoc_json(&self, package: &str) -> Result<rustdoc_types::Crate> {
        build_rustdoc_json(self.path.join(CARGO_TOML), package)
    }
}
