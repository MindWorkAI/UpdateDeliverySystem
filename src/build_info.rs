use std::io::{self, IsTerminal, Write};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const BUILD: &str = env!("UDS_BUILD");
pub const CLAP_VERSION: &str = env!("UDS_CLAP_VERSION");
pub const CHANGELOG: &str = include_str!("../CHANGELOG.md");

pub fn display_version() -> String {
    format!("UDS v{VERSION} (build {BUILD})")
}

pub fn clap_version() -> String {
    CLAP_VERSION.to_owned()
}

pub fn banner() -> String {
    format!(
        " _   _ ____  ____  \n| | | |  _ \\/ ___|\n| | | | | | \\___ \\\n| |_| | |_| |___) |\n \\___/|____/|____/ \n\nMindWork AI Studio · Update Delivery System\nv{VERSION} · build {BUILD}\n\n"
    )
}

pub fn print_banner_if_interactive() -> io::Result<()> {
    let mut stdout = io::stdout();
    if should_print_banner(stdout.is_terminal()) {
        stdout.write_all(banner().as_bytes())?;
        stdout.flush()?;
    }
    Ok(())
}

fn should_print_banner(stdout_is_terminal: bool) -> bool {
    stdout_is_terminal
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_and_banner_contain_build_information() {
        assert_eq!(display_version(), "UDS v26.7.1 (build 1)");
        assert_eq!(clap_version(), "26.7.1 (build 1)");
        let output = banner();
        assert_eq!(
            output,
            " _   _ ____  ____  \n| | | |  _ \\/ ___|\n| | | | | | \\___ \\\n| |_| | |_| |___) |\n \\___/|____/|____/ \n\nMindWork AI Studio · Update Delivery System\nv26.7.1 · build 1\n"
        );
        assert!(output.contains("MindWork AI Studio · Update Delivery System"));
        assert!(output.contains("v26.7.1 · build 1"));
    }

    #[test]
    fn banner_is_only_selected_for_a_terminal() {
        assert!(should_print_banner(true));
        assert!(!should_print_banner(false));
    }
}
