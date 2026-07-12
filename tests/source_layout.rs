//! Regression tests for the build-time UDS source-layout checker.

#[path = "../build_support/source_layout.rs"]
mod source_layout;

use std::{fs, path::PathBuf};

use source_layout::{check_roots, check_source, format_report};

/// Accepts documented fields and variants separated by one blank line.
#[test]
fn accepts_correctly_separated_documented_members() {
    let source = include_str!("fixtures/source_layout/documented.rs.txt");

    assert!(check_source("src/good.rs", source).is_empty());
}

/// Accepts attributes and multiline declarations kept in their visual blocks.
#[test]
fn accepts_attributes_and_multiline_members() {
    let source = include_str!("fixtures/source_layout/attributes.rs.txt");

    assert!(check_source("src/good.rs", source).is_empty());
}

/// Ignores documentation attached to functions, modules, and the data type itself.
#[test]
fn ignores_non_member_documentation() {
    let source = include_str!("fixtures/source_layout/non_members.rs.txt");

    assert!(check_source("src/good.rs", source).is_empty());
}

/// Reports every bad member position in stable source order with useful details.
#[test]
fn reports_multiple_violations_in_order() {
    let source = include_str!("fixtures/source_layout/violations.rs.txt");

    let violations = check_source("src/record.rs", source);

    assert_eq!(violations.len(), 3);
    assert_eq!(violations[0].path, "src/record.rs");
    assert_eq!(violations[0].line, 4);
    assert!(
        violations[0]
            .description
            .contains("found 0 separating line(s)")
    );
    assert_eq!(violations[1].line, 8);
    assert!(
        violations[1]
            .description
            .contains("must stay together without blank lines")
    );
    assert_eq!(violations[2].line, 8);
    assert!(
        violations[2]
            .description
            .contains("found 2 separating line(s)")
    );
}

/// Scans recursive roots in path order and formats file, line, and total details.
#[test]
fn scans_roots_and_formats_a_complete_sorted_report() {
    let root = temporary_test_directory();
    let nested = root.join("src/nested");
    fs::create_dir_all(&nested).expect("fixture directory should be created");
    fs::write(
        root.join("src/z.rs"),
        include_str!("fixtures/source_layout/violations.rs.txt"),
    )
    .expect("top-level fixture should be written");
    fs::write(
        nested.join("a.rs"),
        include_str!("fixtures/source_layout/violations.rs.txt"),
    )
    .expect("nested fixture should be written");

    let violations = check_roots(&root, &["src"]).expect("fixture tree should be checked");
    let report = format_report(&violations);

    assert_eq!(violations.len(), 6);
    assert_eq!(violations[0].path, "src/nested/a.rs");
    assert_eq!(violations[0].line, 4);
    assert_eq!(violations[3].path, "src/z.rs");
    assert!(report.starts_with("src/nested/a.rs:4:"));
    assert!(report.ends_with("source layout check failed with 6 violation(s)\n"));

    fs::remove_dir_all(root).expect("fixture directory should be removed");
}

/// Returns a process-specific scratch directory for recursive checker tests.
fn temporary_test_directory() -> PathBuf {
    std::env::temp_dir().join(format!("uds-source-layout-test-{}", std::process::id()))
}
