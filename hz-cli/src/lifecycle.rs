#![allow(unused_imports)]

use crate::*;
use std::{
    collections::HashSet,
    env,
    ffi::{OsStr, OsString},
    fs,
    io::{self, IsTerminal, Read, Write},
    path::{Path, PathBuf},
    process::{Command as ProcessCommand, ExitCode, Stdio},
    sync::Arc,
};

use clap::{
    Args, Parser, Subcommand, ValueEnum,
    builder::styling::{AnsiColor, Styles},
};
use crossterm::terminal as crossterm_terminal;
use hz_core::HzResult;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

pub(crate) fn run_lifecycle(args: LifecycleArgs, kind: hz_command::LifecycleKind) -> HzResult<()> {
    let run = hz_command::run_lifecycle(hz_command::RunLifecycle {
        target: args.target,
        repo: args.repo,
        kind,
    })?;
    print!("{}", render_lifecycle_run(&run, io::stdout().is_terminal()));
    Ok(())
}
