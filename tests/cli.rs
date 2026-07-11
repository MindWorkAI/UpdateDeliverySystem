use std::process::Command;

#[test]
fn bare_uds_prints_help_and_exits_successfully() {
    let output = Command::new(env!("CARGO_BIN_EXE_uds")).output().unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("Usage: uds [COMMAND]"));
    assert!(stdout.contains("server"));
    assert!(stdout.contains("client"));
}
