use std::process::Command;

#[test]
fn bare_uds_prints_help_and_exits_successfully() {
    let output = Command::new(env!("CARGO_BIN_EXE_uds")).output().unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("Usage: uds [COMMAND]"));
    assert!(stdout.contains("server"));
    assert!(stdout.contains("client"));
    assert!(stdout.contains("version"));
    assert!(stdout.contains("changelog"));
    assert!(!stdout.contains("MindWork AI Studio Update Delivery System"));
}

#[test]
fn version_command_prints_formatted_version() {
    let output = Command::new(env!("CARGO_BIN_EXE_uds"))
        .arg("version")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "UDS v26.7.1 (build 1)\n"
    );
}

#[test]
fn version_flag_includes_build() {
    let output = Command::new(env!("CARGO_BIN_EXE_uds"))
        .arg("--version")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "uds 26.7.1 (build 1)\n"
    );
}

#[test]
fn non_tty_changelog_prints_only_latest_section() {
    let output = Command::new(env!("CARGO_BIN_EXE_uds"))
        .arg("changelog")
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.starts_with("# UDS v26.7.1"));
    assert!(stdout.contains("Initial version"));
    assert_eq!(stdout.matches("# UDS v").count(), 1);
}
