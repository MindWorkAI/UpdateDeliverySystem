//! Build-time integration that exposes the UDS build number to the executable.
use std::{env, fs, path::Path};

#[path = "build_support/source_layout.rs"]
mod source_layout;

fn main() {
    println!("cargo:rerun-if-changed=Cargo.toml");
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=tests");

    let manifest_directory = env::var("CARGO_MANIFEST_DIR").unwrap();
    let manifest_directory = Path::new(&manifest_directory);

    // Report every source-layout problem in one deterministic batch so a
    // developer can fix the complete set before rebuilding.
    let violations = source_layout::check_roots(manifest_directory, &["src", "tests"])
        .expect("failed to inspect Rust sources for UDS layout rules");
    if !violations.is_empty() {
        eprint!("{}", source_layout::format_report(&violations));

        panic!(
            "source layout check failed with {} violation(s)",
            violations.len()
        );
    }

    // Read the manifest explicitly because the UDS build number lives in
    // package metadata instead of Cargo's standard package fields.
    let manifest_path = manifest_directory.join("Cargo.toml");
    let manifest = fs::read_to_string(manifest_path).expect("failed to read Cargo.toml");

    let metadata = manifest
        .split("[package.metadata.uds]")
        .nth(1)
        .expect("Cargo.toml must contain [package.metadata.uds]");
    let build = metadata
        .lines()
        .take_while(|line| !line.trim_start().starts_with('['))
        .find_map(|line| {
            line.split_once('=')
                .filter(|(key, _)| key.trim() == "build")
        })
        .map(|(_, value)| {
            value
                .trim()
                .parse::<u64>()
                .expect("UDS build must be a positive integer")
        })
        .filter(|build| *build > 0)
        .expect("UDS build must be a positive integer");
    println!("cargo:rustc-env=UDS_BUILD={build}");
    println!(
        "cargo:rustc-env=UDS_CLAP_VERSION={} (build {build})",
        env::var("CARGO_PKG_VERSION").expect("Cargo did not provide the package version")
    );
}
