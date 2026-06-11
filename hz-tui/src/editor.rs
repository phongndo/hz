use std::{
    env, io,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, ExitStatus},
    time::Duration,
};

use crossterm::{
    cursor::Show,
    event::{self, DisableMouseCapture, EnableMouseCapture},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use hz_core::HzResult;

const EDITOR_EVENT_DRAIN_LIMIT: usize = 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EditorTarget {
    pub(crate) path: PathBuf,
    pub(crate) line: usize,
}

pub(crate) fn configured_editor() -> Option<String> {
    ["VISUAL", "GIT_EDITOR", "EDITOR"]
        .into_iter()
        .filter_map(env::var_os)
        .map(|editor| editor.to_string_lossy().trim().to_owned())
        .find(|editor| !editor.is_empty())
}

pub(crate) fn repo_file_path(repo: &Path, path: &str) -> PathBuf {
    let path = Path::new(path);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        repo.join(path)
    }
}

pub(crate) fn open_editor(editor: &str, target: &EditorTarget) -> HzResult<ExitStatus> {
    let mut terminal = SuspendedTerminal::suspend()?;
    let status_result = editor_status(editor, target);
    terminal.resume()?;
    Ok(status_result?)
}

pub(crate) fn editor_status(editor: &str, target: &EditorTarget) -> io::Result<ExitStatus> {
    let Some(parts) = split_editor_command(editor) else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "editor command is empty or invalid",
        ));
    };

    let mut command = Command::new(&parts[0]);
    command.args(&parts[1..]);
    if editor_uses_goto_arg(&parts[0]) {
        command.arg("--goto");
        command.arg(format!("{}:{}", target.path.display(), target.line.max(1)));
    } else {
        command.arg(format!("+{}", target.line.max(1)));
        command.arg(&target.path);
    }
    attach_terminal_stdio(&mut command)?;

    command.status()
}

pub(crate) fn split_editor_command(editor: &str) -> Option<Vec<String>> {
    let parts = shlex::split(editor)?;
    (!parts.is_empty()).then_some(parts)
}

pub(crate) fn editor_uses_goto_arg(program: &str) -> bool {
    let name = Path::new(program)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(program)
        .to_ascii_lowercase();
    matches!(
        name.as_str(),
        "code" | "code-insiders" | "codium" | "cursor"
    )
}

struct SuspendedTerminal {
    active: bool,
}

impl SuspendedTerminal {
    fn suspend() -> HzResult<Self> {
        let terminal = Self { active: true };
        disable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, DisableMouseCapture, LeaveAlternateScreen, Show)?;
        stdout.flush()?;
        Ok(terminal)
    }

    fn resume(&mut self) -> HzResult<()> {
        if !self.active {
            return Ok(());
        }

        let _ = flush_terminal_input_queue();
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
        stdout.flush()?;
        enable_raw_mode()?;
        drain_pending_editor_events()?;
        self.active = false;
        Ok(())
    }
}

fn drain_pending_editor_events() -> io::Result<()> {
    for _ in 0..EDITOR_EVENT_DRAIN_LIMIT {
        if !event::poll(Duration::ZERO)? {
            break;
        }

        let _ = event::read()?;
    }

    Ok(())
}

#[cfg(unix)]
fn flush_terminal_input_queue() -> io::Result<()> {
    use std::fs::OpenOptions;

    use rustix::{
        io::Errno,
        termios::{QueueSelector, isatty, tcflush},
    };

    let stdin = io::stdin();
    let flush_result = if isatty(&stdin) {
        tcflush(stdin, QueueSelector::IFlush)
    } else {
        let tty = OpenOptions::new().read(true).write(true).open("/dev/tty")?;
        tcflush(tty, QueueSelector::IFlush)
    };

    match flush_result {
        Ok(()) | Err(Errno::NOTTY) => Ok(()),
        Err(error) => Err(error.into()),
    }
}

#[cfg(not(unix))]
fn flush_terminal_input_queue() -> io::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn attach_terminal_stdio(command: &mut Command) -> io::Result<()> {
    use std::fs::OpenOptions;
    use std::process::Stdio;

    let tty = OpenOptions::new().read(true).write(true).open("/dev/tty")?;
    command.stdin(Stdio::from(tty.try_clone()?));
    command.stdout(Stdio::from(tty.try_clone()?));
    command.stderr(Stdio::from(tty));
    Ok(())
}

#[cfg(not(unix))]
fn attach_terminal_stdio(_command: &mut Command) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "opening editors from the TUI is unsupported on this platform",
    ))
}

impl Drop for SuspendedTerminal {
    fn drop(&mut self) {
        let _ = self.resume();
    }
}
