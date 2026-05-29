use std::{
    env, fs,
    path::{Path, PathBuf},
};

use hz_core::{HzError, HzResult};

pub use hz_diff::DiffOptions;
pub use hz_worktree::{
    CreateWorktree, CreatedWorktree, FindWorktree, HandoffMode, HandoffWorktree, ListWorktrees,
    PathWorktree, RemoveWorktree, WorktreeEntry, WorktreeHandoff, WorktreeSource, WorktreeStatus,
};

pub fn create_worktree(input: CreateWorktree) -> HzResult<CreatedWorktree> {
    hz_worktree::create(input)
}

pub fn path_worktree(input: PathWorktree) -> HzResult<hz_core::paths::WorktreeTarget> {
    hz_worktree::path(input)
}

pub fn handoff_worktree(input: HandoffWorktree) -> HzResult<WorktreeHandoff> {
    hz_worktree::handoff(input)
}

pub fn list_worktrees(input: ListWorktrees) -> HzResult<Vec<WorktreeEntry>> {
    hz_worktree::list(input)
}

pub fn find_worktree(input: FindWorktree) -> HzResult<WorktreeEntry> {
    hz_worktree::find(input)
}

pub fn remove_worktree(input: RemoveWorktree) -> HzResult<WorktreeEntry> {
    hz_worktree::remove(input)
}

pub fn remove_found_worktree(entry: WorktreeEntry) -> HzResult<WorktreeEntry> {
    hz_worktree::remove_found(entry)
}

pub fn remove_found_worktree_with_force(
    entry: WorktreeEntry,
    force: bool,
) -> HzResult<WorktreeEntry> {
    hz_worktree::remove_found_with_force(entry, force)
}

pub fn diff(input: DiffOptions) -> HzResult<String> {
    hz_diff::render(input)
}

pub fn shell_init_line(shell: Shell) -> &'static str {
    match shell {
        Shell::Zsh => r#"eval "$(hz shell zsh)""#,
        Shell::Bash => r#"eval "$(hz shell bash)""#,
        Shell::Fish => "hz shell fish | source",
    }
}

pub fn shell_init_comment() -> &'static str {
    "# hz shell integration"
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellInit {
    pub path: PathBuf,
    pub line: &'static str,
    pub changed: bool,
}

pub fn install_shell_integration(shell: Shell) -> HzResult<ShellInit> {
    let path = shell_rc_path(shell)?;
    let line = shell_init_line(shell);
    let changed = install_line(&path, line)?;

    Ok(ShellInit {
        path,
        line,
        changed,
    })
}

pub fn shell_integration(shell: Shell) -> &'static str {
    match shell {
        Shell::Zsh => include_str!("shell/hz.zsh"),
        Shell::Bash => include_str!("shell/hz.bash"),
        Shell::Fish => include_str!("shell/hz.fish"),
    }
}

fn shell_rc_path(shell: Shell) -> HzResult<PathBuf> {
    let home = home_dir()?;
    match shell {
        Shell::Zsh => Ok(home.join(".zshrc")),
        Shell::Bash => Ok(home.join(".bashrc")),
        Shell::Fish => {
            let config_home = match env::var_os("XDG_CONFIG_HOME") {
                Some(path) => PathBuf::from(path),
                None => home.join(".config"),
            };
            Ok(config_home.join("fish").join("config.fish"))
        }
    }
}

fn install_line(path: &Path, line: &'static str) -> HzResult<bool> {
    let existing = if path.exists() {
        fs::read_to_string(path)?
    } else {
        String::new()
    };

    if existing.lines().any(|existing_line| existing_line == line) {
        return Ok(false);
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut next = existing;
    if !next.is_empty() && !next.ends_with('\n') {
        next.push('\n');
    }
    next.push_str(shell_init_comment());
    next.push('\n');
    next.push_str(line);
    next.push('\n');

    fs::write(path, next)?;
    Ok(true)
}

fn home_dir() -> HzResult<PathBuf> {
    env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| HzError::Usage("HOME is not set".to_owned()))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shell {
    Zsh,
    Bash,
    Fish,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zsh_init_line_is_rc_file_friendly() {
        assert_eq!(shell_init_line(Shell::Zsh), r#"eval "$(hz shell zsh)""#);
    }

    #[test]
    fn zsh_integration_wraps_new_and_cd() {
        let script = shell_integration(Shell::Zsh);

        assert!(script.contains("command hz \"$@\" --path-only"));
        assert!(script.contains("handoff)"));
        assert!(script.contains("--json|--path-only|--help|-h|-j"));
        assert!(script.contains("builtin cd \"$hz_target_path\" || return"));
    }

    #[test]
    fn fish_integration_passes_json_short_flag_through() {
        let script = shell_integration(Shell::Fish);

        assert!(script.contains("case --json --path-only --help -h -j"));
        assert!(script.contains("or return"));
    }

    #[test]
    fn installs_line_once() {
        let test_dir = env::temp_dir().join(format!(
            "hz-init-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time should be after unix epoch")
                .as_nanos()
        ));
        let rc_file = test_dir.join(".zshrc");

        assert!(install_line(&rc_file, shell_init_line(Shell::Zsh)).unwrap());
        assert!(!install_line(&rc_file, shell_init_line(Shell::Zsh)).unwrap());

        let contents = fs::read_to_string(&rc_file).unwrap();
        assert_eq!(contents.matches(shell_init_line(Shell::Zsh)).count(), 1);
        assert_eq!(contents.matches(shell_init_comment()).count(), 1);

        fs::remove_dir_all(test_dir).unwrap();
    }

    #[test]
    fn does_not_duplicate_existing_bare_line() {
        let test_dir = env::temp_dir().join(format!(
            "hz-init-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time should be after unix epoch")
                .as_nanos()
        ));
        let rc_file = test_dir.join(".zshrc");
        fs::create_dir_all(&test_dir).unwrap();
        fs::write(&rc_file, format!("{}\n", shell_init_line(Shell::Zsh))).unwrap();

        assert!(!install_line(&rc_file, shell_init_line(Shell::Zsh)).unwrap());

        let contents = fs::read_to_string(&rc_file).unwrap();
        assert_eq!(contents.matches(shell_init_line(Shell::Zsh)).count(), 1);
        assert_eq!(contents.matches(shell_init_comment()).count(), 0);

        fs::remove_dir_all(test_dir).unwrap();
    }
}
