//! Compile-time UDS version metadata and startup banner rendering.

use std::io::{self, IsTerminal, Write};

/// Defines the VERSION value exposed by UDS.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Defines the BUILD value exposed by UDS.
pub const BUILD: &str = env!("UDS_BUILD");

/// Defines the CLAP VERSION value exposed by UDS.
pub const CLAP_VERSION: &str = env!("UDS_CLAP_VERSION");

/// Defines the CHANGELOG value exposed by UDS.
pub const CHANGELOG: &str = include_str!("../CHANGELOG.md");

/// Produces the display version representation returned or displayed by UDS.
pub fn display_version() -> String {
    format!("UDS v{VERSION} (build {BUILD})")
}

/// Provides the clap version operation used by UDS callers.
pub fn clap_version() -> String {
    CLAP_VERSION.to_owned()
}

/// Provides the banner operation used by UDS callers.
pub fn banner() -> String {
    format!(
        r#"
    _   _ ____  ____
   | | | |  _ \/ ___|
   | | | | | | \___ \
   | |_| | |_| |___) |
    \___/|____/|____/

   MindWork AI Studio · Update Delivery System
   v{VERSION} · build {BUILD}

"#
    )
}

/// Provides the print banner if interactive operation used by UDS callers.
pub fn print_banner_if_interactive() -> io::Result<()> {
    let mut stdout = io::stdout();
    if should_print_banner(stdout.is_terminal()) {
        stdout.write_all(banner().as_bytes())?;
        stdout.flush()?;
    }
    Ok(())
}

/// Performs the should print banner operation required by UDS.
fn should_print_banner(stdout_is_terminal: bool) -> bool {
    stdout_is_terminal
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies that version and banner contain build information.
    #[test]
    fn version_and_banner_contain_build_information() {
        assert_eq!(display_version(), "UDS v26.7.1 (build 1)");
        assert_eq!(clap_version(), "26.7.1 (build 1)");
        let output = banner();
        assert_eq!(
            output,
            r#"
    _   _ ____  ____
   | | | |  _ \/ ___|
   | | | | | | \___ \
   | |_| | |_| |___) |
    \___/|____/|____/

   MindWork AI Studio · Update Delivery System
   v26.7.1 · build 1

"#
        );
        assert!(output.contains("MindWork AI Studio · Update Delivery System"));
        assert!(output.contains("v26.7.1 · build 1"));
    }

    /// Verifies that banner is only selected for a terminal.
    #[test]
    fn banner_is_only_selected_for_a_terminal() {
        assert!(should_print_banner(true));
        assert!(!should_print_banner(false));
    }
}
