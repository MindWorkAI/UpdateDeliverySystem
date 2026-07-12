//! Checks the UDS-specific spacing of documented and annotated data members.

use std::{fs, io, path::Path};

/// One source-layout problem found while scanning Rust sources.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Violation {
    /// Repository-relative path containing the problem.
    pub path: String,

    /// One-based line on which the affected member block starts.
    pub line: usize,

    /// Human-readable explanation of the required layout.
    pub description: String,
}

/// Formats a deterministic batch report, including its total violation count.
pub fn format_report(violations: &[Violation]) -> String {
    let mut report = String::new();

    for violation in violations {
        report.push_str(&format!(
            "{}:{}: {}\n",
            violation.path, violation.line, violation.description
        ));
    }

    report.push_str(&format!(
        "source layout check failed with {} violation(s)\n",
        violations.len()
    ));

    report
}

/// Recursively checks Rust files below each supplied repository-relative root.
pub fn check_roots(manifest_dir: &Path, roots: &[&str]) -> io::Result<Vec<Violation>> {
    let mut files = Vec::new();

    for root in roots {
        collect_rust_files(&manifest_dir.join(root), &mut files)?;
    }

    files.sort();

    let mut violations = Vec::new();
    for file in files {
        let source = fs::read_to_string(&file)?;
        let relative = file
            .strip_prefix(manifest_dir)
            .unwrap_or(&file)
            .to_string_lossy()
            .replace('\\', "/");
        violations.extend(check_source(&relative, &source));
    }
    violations.sort();

    Ok(violations)
}

/// Checks one Rust source string for spacing violations.
pub fn check_source(path: &str, source: &str) -> Vec<Violation> {
    let lines: Vec<&str> = source.lines().collect();
    let mut violations = Vec::new();
    let mut container_depth = None;
    let mut pending_container = false;
    let mut brace_depth = 0_i32;
    let mut block_start = None;
    let mut block_decorated = false;
    let mut attribute_nesting = 0_i32;
    let mut member_started = false;
    let mut member_nesting = 0_i32;
    let mut previous_member: Option<(usize, bool)> = None;

    for (index, line) in lines.iter().enumerate() {
        let line_number = index + 1;
        let code = code_before_line_comment(line);
        let trimmed = code.trim();
        let source_trimmed = line.trim();
        let depth_at_start = brace_depth;

        if container_depth.is_none() && (pending_container || starts_braced_data_type(trimmed)) {
            pending_container = !code.contains('{');
            if code.contains('{') {
                container_depth = Some(depth_at_start + 1);
                pending_container = false;
                block_start = None;
                block_decorated = false;
                attribute_nesting = 0;
                member_started = false;
                previous_member = None;
            }
        }

        if let Some(member_depth) = container_depth
            && depth_at_start == member_depth
            && !source_trimmed.is_empty()
            && !source_trimmed.starts_with('}')
        {
            let decoration = source_trimmed.starts_with("///") || source_trimmed.starts_with("#[");

            if !member_started && (decoration || block_start.is_some()) {
                block_start.get_or_insert(line_number);
                block_decorated |= decoration;
            }

            let attribute_line = !member_started && (attribute_nesting > 0 || source_trimmed.starts_with("#["));
            if attribute_line {
                attribute_nesting += delimiter_delta(code);
            }

            if !member_started && !is_block_prefix(source_trimmed) && !attribute_line {
                let current_start = block_start.unwrap_or(line_number);
                if block_start.is_some()
                    && lines[current_start - 1..line_number - 1]
                        .iter()
                        .any(|line| line.trim().is_empty())
                {
                    violations.push(Violation {
                            path: path.to_owned(),
                            line: current_start,
                            description: "documentation, attributes, and the member declaration must stay together without blank lines"
                                .to_owned(),
                        });
                }

                if let Some((previous_end, previous_decorated)) = previous_member
                    && (previous_decorated || block_decorated)
                {
                    let separating_lines = current_start.saturating_sub(previous_end + 1);
                    let exactly_one_blank = separating_lines == 1 && lines[previous_end].trim().is_empty();

                    if !exactly_one_blank {
                        violations.push(Violation {
                                path: path.to_owned(),
                                line: current_start,
                                description: format!(
                                    "expected exactly one blank line before this documented or annotated struct field or enum variant; found {separating_lines} separating line(s)"
                                ),
                            });
                    }
                }

                member_started = true;
                member_nesting = delimiter_delta(code);
            } else if member_started {
                member_nesting += delimiter_delta(code);
            }

            if member_started && member_nesting <= 0 && code.contains(',') {
                previous_member = Some((line_number, block_decorated));
                block_start = None;
                block_decorated = false;
                attribute_nesting = 0;
                member_started = false;
                member_nesting = 0;
            }
        }

        brace_depth += brace_delta(code);

        if let Some(member_depth) = container_depth
            && brace_depth < member_depth
        {
            container_depth = None;
            block_start = None;
            block_decorated = false;
            attribute_nesting = 0;
            member_started = false;
            previous_member = None;
        }
    }

    violations
}

/// Adds all Rust files below one source root to the shared work list.
fn collect_rust_files(directory: &Path, files: &mut Vec<std::path::PathBuf>) -> io::Result<()> {
    if !directory.exists() {
        return Ok(());
    }

    let mut entries = fs::read_dir(directory)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.path());

    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            collect_rust_files(&path, files)?;
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            files.push(path);
        }
    }

    Ok(())
}

/// Recognizes a braced struct or enum declaration without matching type aliases.
fn starts_braced_data_type(line: &str) -> bool {
    let words: Vec<&str> = line
        .split(|character: char| !character.is_alphanumeric() && character != '_')
        .collect();

    words.contains(&"struct") || words.contains(&"enum")
}

/// Returns whether a top-level line still belongs to documentation or attributes.
fn is_block_prefix(line: &str) -> bool {
    line.starts_with("///")
        || line.starts_with("#[")
        || line.starts_with("//")
        || line.starts_with("/*")
        || line.starts_with('*')
        || line.starts_with("*/")
}

/// Removes a line comment so braces in prose cannot affect nesting.
fn code_before_line_comment(line: &str) -> &str {
    line.split_once("//").map_or(line, |(code, _)| code)
}

/// Counts curly braces on a source line after line comments were removed.
fn brace_delta(line: &str) -> i32 {
    line.chars().fold(0, |depth, character| match character {
        '{' => depth + 1,
        '}' => depth - 1,
        _ => depth,
    })
}

/// Counts all delimiters used by a potentially multiline member declaration.
fn delimiter_delta(line: &str) -> i32 {
    line.chars().fold(0, |depth, character| match character {
        '(' | '[' | '{' | '<' => depth + 1,
        ')' | ']' | '}' | '>' => depth - 1,
        _ => depth,
    })
}
