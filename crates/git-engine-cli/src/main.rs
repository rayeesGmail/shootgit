// CLAUDE.md: `unwrap`/`expect` are allowed in `main.rs` and tests only.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! `git-engine-cli`: see the library docs for the commands and their output.

use std::io;
use std::process::ExitCode;

use git_engine_cli::{describe, parse_args, run, USAGE};

/// A command line that does not parse (the usual convention).
const USAGE_ERROR: u8 = 2;

fn main() -> ExitCode {
    let command = match parse_args(std::env::args_os().skip(1)) {
        Ok(command) => command,
        Err(error) => {
            eprintln!("error: {error}\n\n{USAGE}");
            return ExitCode::from(USAGE_ERROR);
        }
    };
    match run(command, &mut io::stdout(), &mut io::stderr()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) if error.is_closed_output() => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {}", describe(&error));
            ExitCode::FAILURE
        }
    }
}
