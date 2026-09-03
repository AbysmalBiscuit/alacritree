//! Replay a `conpty_stream` recording through the real VT parser and report how
//! much of the time the watched row was on screen.
//!
//! The recording is what the pseudoconsole sent.  Measuring presence in it
//! separates "conpty never sent the row" from "we lost it", which a grid
//! sample alone cannot do.  The parser here is `alacritty_terminal`'s own, so
//! the answer does not depend on a hand-written model of the escape sequences.
//!
//! Usage: `stream_presence <log> [--match TEXT] [--cols N] [--rows N]`

use alacritty_terminal::event::VoidListener;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::{Config as TermConfig, Term};
use alacritty_terminal::vte::ansi::{Processor, StdSyncHandler};

#[derive(Copy, Clone)]
struct Size {
    columns: usize,
    screen_lines: usize,
}

impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        self.screen_lines
    }
    fn screen_lines(&self) -> usize {
        self.screen_lines
    }
    fn columns(&self) -> usize {
        self.columns
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("a log path");
    let mut needle = String::from("Building");
    let mut size = Size { columns: 117, screen_lines: 30 };
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--match" => needle = args.next().expect("text"),
            "--cols" => size.columns = args.next().expect("n").parse().expect("number"),
            "--rows" => size.screen_lines = args.next().expect("n").parse().expect("number"),
            other => panic!("unexpected argument {other}"),
        }
    }

    let text = std::fs::read_to_string(&path).expect("read the log");
    let mut term = Term::new(TermConfig::default(), &size, VoidListener);
    let mut parser = Processor::<StdSyncHandler>::new();

    // Each record carries the time its read arrived, so a state holds until the
    // next record rather than for an equal share of the run.
    let mut records: Vec<(f64, Vec<u8>)> = Vec::new();
    for line in text.lines() {
        let mut fields = line.split('\t');
        let micros: f64 = fields.next().expect("a timestamp").parse().expect("number");
        let _len = fields.next();
        let hex = fields.next().unwrap_or("");
        let bytes = (0..hex.len() / 2)
            .map(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).expect("hex"))
            .collect();
        records.push((micros / 1e6, bytes));
    }

    let mut present_for = 0.0;
    let mut observed = 0.0;
    let mut first_match: Option<f64> = None;
    let mut longest_gap: f64 = 0.0;
    let mut gap_run = 0.0;
    let mut off_column = 0.0;
    let mut longest_off: f64 = 0.0;
    let mut off_run = 0.0;

    for i in 0..records.len() {
        let (at, bytes) = &records[i];
        parser.advance(&mut term, bytes);
        let dt = records.get(i + 1).map_or(0.0, |(next, _)| next - at);

        let grid = term.grid();
        let mut present = false;
        for line in 0..grid.screen_lines() {
            let row = &grid[Line(line as i32)];
            let row_text: String = (0..grid.columns()).map(|c| row[Column(c)].c).collect();
            if row_text.contains(&needle) {
                present = true;
                break;
            }
        }
        let cursor_column = grid.cursor.point.column.0;

        if present && first_match.is_none() {
            first_match = Some(*at);
        }
        if first_match.is_none() {
            continue;
        }
        observed += dt;
        if present {
            present_for += dt;
            gap_run = 0.0;
        } else {
            gap_run += dt;
            longest_gap = longest_gap.max(gap_run);
        }
        if cursor_column == 0 {
            off_run = 0.0;
        } else {
            off_column += dt;
            off_run += dt;
            longest_off = longest_off.max(off_run);
        }
    }

    let pct = |v: f64| if observed > 0.0 { 100.0 * v / observed } else { 0.0 };
    println!(
        "{} reads, first match at {:.2} s, {:.1} s observed after that",
        records.len(),
        first_match.unwrap_or(0.0),
        observed
    );
    println!(
        "{needle:?} present {:.2}% of the stream, longest gap {:.1} ms",
        pct(present_for),
        longest_gap * 1000.0
    );
    println!(
        "cursor off column 0 {:.3}% of the stream, longest {:.1} ms",
        pct(off_column),
        longest_off * 1000.0
    );


}
