use std::{
    env, io,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
};

use crossterm::{
    cursor::Show,
    event::{DisableMouseCapture, EnableMouseCapture},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use hz_core::HzResult;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EditorTarget {
    pub(crate) path: PathBuf,
    pub(crate) line: usize,
}

pub(crate) fn configured_editor() -> Option<String> {
    ["VISUAL", "EDITOR", "editor"]
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
            "editor command is empty",
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
    let parts: Vec<_> = editor
        .split_whitespace()
        .map(str::to_owned)
        .filter(|part| !part.is_empty())
        .collect();
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

        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
        stdout.flush()?;
        enable_raw_mode()?;
        self.active = false;
        Ok(())
    }
}

#[cfg(unix)]
fn attach_terminal_stdio(command: &mut Command) -> io::Result<()> {
    use std::fs::OpenOptions;

    let tty = OpenOptions::new().read(true).write(true).open("/dev/tty")?;
    command.stdin(Stdio::from(tty.try_clone()?));
    command.stdout(Stdio::from(tty.try_clone()?));
    command.stderr(Stdio::from(tty));
    Ok(())
}

#[cfg(not(unix))]
fn attach_terminal_stdio(_command: &mut Command) -> io::Result<()> {
    Ok(())
}

impl Drop for SuspendedTerminal {
    fn drop(&mut self) {
        let _ = self.resume();
    }
}
