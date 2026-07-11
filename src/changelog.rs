//! Parsing and terminal browsing for the embedded UDS changelog.
//!
//! Embedding the changelog gives operators release information even when the
//! server host has no browser or network access.

use std::io::{self, IsTerminal, Write};
use std::time::Duration;

use anyhow::{Context, bail};
use crossterm::{
    cursor::{Hide, MoveTo, Show},
    event::{self, Event, KeyCode, KeyEventKind},
    execute, queue,
    style::Print,
    terminal::{self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen},
};
use semver::Version;

use crate::build_info;

#[derive(Debug, Clone, PartialEq, Eq)]
/// Parsed changelog section for one released UDS version.
pub struct Section<'a> {
    /// The version carried by this UDS data contract.
    pub version: Version,

    /// The markdown carried by this UDS data contract.
    pub markdown: &'a str,
}

/// Provides the parse operation used by UDS callers.
pub fn parse<'a>(source: &'a str, expected: &str) -> anyhow::Result<Vec<Section<'a>>> {
    let mut starts = Vec::new();
    for (offset, line) in line_offsets(source) {
        if line.starts_with("# ") {
            let value = line
                .strip_prefix("# UDS v")
                .with_context(|| format!("invalid level-1 changelog heading: {line}"))?;
            let version = Version::parse(value).with_context(|| format!("invalid changelog version: {value}"))?;
            starts.push((offset, version));
        } else if line.starts_with('#') && !line.starts_with("##") {
            bail!("invalid level-1 changelog heading: {line}");
        }
    }
    if starts.is_empty() {
        bail!("changelog contains no release sections");
    }
    for pair in starts.windows(2) {
        if pair[0].1 <= pair[1].1 {
            bail!("changelog versions must be unique and newest first");
        }
    }
    if starts[0].1 != Version::parse(expected)? {
        bail!("first changelog version must match Cargo package version {expected}");
    }
    Ok(starts
        .iter()
        .enumerate()
        .map(|(index, (start, version))| {
            let end = starts.get(index + 1).map_or(source.len(), |entry| entry.0);
            Section {
                version: version.clone(),
                markdown: source[*start..end].trim_end(),
            }
        })
        .collect())
}

/// Performs the line offsets operation required by UDS.
fn line_offsets(source: &str) -> impl Iterator<Item = (usize, &str)> {
    let mut offset = 0;
    source.split_inclusive('\n').map(move |line| {
        let start = offset;
        offset += line.len();
        (start, line.trim_end_matches(['\r', '\n']))
    })
}

/// Runs the run workflow for UDS.
pub fn run() -> anyhow::Result<()> {
    let sections = parse(build_info::CHANGELOG, build_info::VERSION)?;
    if !io::stdout().is_terminal() || !io::stdin().is_terminal() {
        println!("{}", sections[0].markdown);
        return Ok(());
    }
    ViewerTerminal::enter()?.run(&sections)
}

#[derive(Debug, Default)]
/// Scroll and selection state for the interactive changelog viewer.
struct ViewerState {
    /// Stores the section value used by this UDS component.
    section: usize,

    /// Stores the scroll value used by this UDS component.
    scroll: usize,
}

impl ViewerState {
    /// Performs the older operation required by UDS.
    fn older(&mut self, count: usize) {
        self.section = (self.section + 1).min(count.saturating_sub(1));
        self.scroll = 0;
    }

    /// Performs the newer operation required by UDS.
    fn newer(&mut self) {
        self.section = self.section.saturating_sub(1);
        self.scroll = 0;
    }

    /// Performs the scroll by operation required by UDS.
    fn scroll_by(&mut self, delta: isize, max: usize) {
        self.scroll = self.scroll.saturating_add_signed(delta).min(max);
    }
}

/// Terminal-mode guard that restores the console when browsing ends.
struct ViewerTerminal;

impl ViewerTerminal {
    /// Performs the enter operation required by UDS.
    fn enter() -> anyhow::Result<Self> {
        terminal::enable_raw_mode()?;
        if let Err(error) = execute!(io::stdout(), EnterAlternateScreen, Hide) {
            let _ = terminal::disable_raw_mode();
            return Err(error.into());
        }
        Ok(Self)
    }

    /// Performs the run operation required by UDS.
    fn run(self, sections: &[Section<'_>]) -> anyhow::Result<()> {
        let mut state = ViewerState::default();
        loop {
            let (width, height) = terminal::size()?;
            let lines = wrapped_lines(sections[state.section].markdown, width as usize);
            let body_height = height.saturating_sub(3) as usize;
            let max_scroll = lines.len().saturating_sub(body_height);
            state.scroll = state.scroll.min(max_scroll);
            draw(&state, sections, &lines, width, height)?;
            if !event::poll(Duration::from_millis(500))? {
                continue;
            }
            match event::read()? {
                Event::Resize(_, _) => {}
                Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => break,
                    KeyCode::Left => state.older(sections.len()),
                    KeyCode::Right => state.newer(),
                    KeyCode::Up => state.scroll_by(-1, max_scroll),
                    KeyCode::Down => state.scroll_by(1, max_scroll),
                    KeyCode::PageUp => state.scroll_by(-(body_height as isize), max_scroll),
                    KeyCode::PageDown => state.scroll_by(body_height as isize, max_scroll),
                    KeyCode::Home => state.scroll = 0,
                    KeyCode::End => state.scroll = max_scroll,
                    _ => {}
                },
                _ => {}
            }
        }
        Ok(())
    }
}

impl Drop for ViewerTerminal {
    fn drop(&mut self) {
        let _ = execute!(io::stdout(), Show, LeaveAlternateScreen);
        let _ = terminal::disable_raw_mode();
    }
}

/// Performs the wrapped lines operation required by UDS.
fn wrapped_lines(markdown: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut output = Vec::new();
    for line in markdown.lines() {
        if line.is_empty() {
            output.push(String::new());
            continue;
        }
        let mut rest = line;
        while rest.chars().count() > width {
            let split = rest
                .char_indices()
                .take_while(|(index, _)| *index <= width)
                .filter(|(_, ch)| ch.is_whitespace())
                .map(|(index, _)| index)
                .last()
                .unwrap_or_else(|| rest.char_indices().nth(width).map_or(rest.len(), |v| v.0));
            output.push(rest[..split].trim_end().to_owned());
            rest = rest[split..].trim_start();
        }
        output.push(rest.to_owned());
    }
    output
}

/// Performs the draw operation required by UDS.
fn draw(state: &ViewerState, sections: &[Section<'_>], lines: &[String], width: u16, height: u16) -> io::Result<()> {
    let mut stdout = io::stdout();
    queue!(stdout, MoveTo(0, 0), Clear(ClearType::All))?;
    let title = format!(
        "UDS changelog — {}  [{}/{}]",
        sections[state.section].version,
        state.section + 1,
        sections.len()
    );
    queue!(stdout, Print(truncate(&title, width as usize)))?;
    for (row, line) in lines
        .iter()
        .skip(state.scroll)
        .take(height.saturating_sub(3) as usize)
        .enumerate()
    {
        queue!(
            stdout,
            MoveTo(0, row as u16 + 2),
            Print(truncate(line, width as usize))
        )?;
    }
    if height > 1 {
        queue!(
            stdout,
            MoveTo(0, height - 1),
            Print(truncate(
                "← older  → newer  ↑↓/PgUp/PgDn/Home/End scroll  q/Esc quit",
                width as usize
            ))
        )?;
    }
    stdout.flush()
}

/// Performs the truncate operation required by UDS.
fn truncate(value: &str, width: usize) -> String {
    value.chars().take(width).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID: &str = "# UDS v2.0.0\n\n## Added\n\nNew.\n# UDS v1.0.0\n\nOld.\n";

    /// Verifies that parses versions and keeps subheadings and blank lines.
    #[test]
    fn parses_versions_and_keeps_subheadings_and_blank_lines() {
        let sections = parse(VALID, "2.0.0").unwrap();
        assert_eq!(sections.len(), 2);
        assert!(sections[0].markdown.contains("## Added\n\nNew."));
    }

    /// Verifies that rejects bad headings duplicates order and version mismatch.
    #[test]
    fn rejects_bad_headings_duplicates_order_and_version_mismatch() {
        assert!(parse("# Wrong\n", "1.0.0").is_err());
        assert!(parse("# UDS v1.0.0\n# UDS v1.0.0\n", "1.0.0").is_err());
        assert!(parse("# UDS v1.0.0\n# UDS v2.0.0\n", "1.0.0").is_err());
        assert!(parse(VALID, "3.0.0").is_err());
    }

    /// Verifies that navigation and scroll stay in bounds.
    #[test]
    fn navigation_and_scroll_stay_in_bounds() {
        let mut state = ViewerState::default();
        state.newer();
        assert_eq!(state.section, 0);
        state.older(2);
        assert_eq!((state.section, state.scroll), (1, 0));
        state.older(2);
        assert_eq!(state.section, 1);
        state.scroll_by(20, 3);
        assert_eq!(state.scroll, 3);
        state.scroll_by(-20, 3);
        assert_eq!(state.scroll, 0);
        state.newer();
        assert_eq!((state.section, state.scroll), (0, 0));
    }

    /// Verifies that wrapping handles tiny terminals.
    #[test]
    fn wrapping_handles_tiny_terminals() {
        assert_eq!(wrapped_lines("abc", 1), ["a", "b", "c"]);
    }
}
