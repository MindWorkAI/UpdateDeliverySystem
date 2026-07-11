use std::{env, fs, path::Path};

fn main() {
    println!("cargo:rerun-if-changed=Cargo.toml");
    let manifest =
        fs::read_to_string(Path::new(&env::var("CARGO_MANIFEST_DIR").unwrap()).join("Cargo.toml"))
            .expect("failed to read Cargo.toml");
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
