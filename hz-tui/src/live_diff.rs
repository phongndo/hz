use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, Sender},
    },
    thread,
};

use hz_core::{HzError, HzResult};
use hz_diff::{Changeset, DiffOptions, DiffSource};
use notify::{RecursiveMode, Watcher};

use crate::theme::LIVE_RELOAD_DEBOUNCE;

pub(crate) struct LiveDiff {
    pub(crate) options: DiffOptions,
    pub(crate) _watcher: notify::RecommendedWatcher,
    pub(crate) _worker: thread::JoinHandle<()>,
    pub(crate) control_tx: Sender<LiveDiffCommand>,
    pub(crate) reload_rx: Receiver<LiveDiffReload>,
    paused: Arc<AtomicBool>,
    pending_while_paused: Arc<AtomicBool>,
}

impl LiveDiff {
    pub(crate) fn start(options: DiffOptions, repo: &Path) -> HzResult<Self> {
        let watch_spec = live_diff_watch_spec(repo)?;
        let filter = watch_spec.filter.clone();
        let (control_tx, control_rx) = mpsc::channel();
        let (reload_tx, reload_rx) = mpsc::channel();
        let watcher_tx = control_tx.clone();
        let paused = Arc::new(AtomicBool::new(false));
        let watcher_paused = Arc::clone(&paused);
        let pending_while_paused = Arc::new(AtomicBool::new(false));
        let watcher_pending_while_paused = Arc::clone(&pending_while_paused);

        let mut watcher =
            notify::recommended_watcher(move |result: Result<notify::Event, notify::Error>| {
                match result {
                    Ok(event) if filter.is_relevant_event(&event) => {
                        queue_changed_or_record_pending(
                            &watcher_paused,
                            &watcher_pending_while_paused,
                            &watcher_tx,
                        );
                    }
                    Ok(_) | Err(_) => {}
                }
            })
            .map_err(|error| watcher_error("failed to start live diff watcher", error))?;

        for watch_path in &watch_spec.watch_paths {
            if !watch_path.path.exists() {
                continue;
            }
            watcher
                .watch(&watch_path.path, watch_path.recursive_mode())
                .map_err(|error| {
                    watcher_error(
                        &format!("failed to watch {}", watch_path.path.display()),
                        error,
                    )
                })?;
        }

        let worker = spawn_live_diff_worker(
            options.clone(),
            control_rx,
            reload_tx,
            Arc::clone(&paused),
            Arc::clone(&pending_while_paused),
        );

        Ok(Self {
            options,
            _watcher: watcher,
            _worker: worker,
            control_tx,
            reload_rx,
            paused,
            pending_while_paused,
        })
    }

    pub(crate) fn set_paused(&self, paused: bool) {
        self.paused.store(paused, Ordering::Release);
        if !paused {
            flush_pending_paused_reload(&self.pending_while_paused, &self.control_tx);
        }
    }
}

impl Drop for LiveDiff {
    fn drop(&mut self) {
        let _ = self.control_tx.send(LiveDiffCommand::Stop);
    }
}

#[derive(Debug)]
pub(crate) enum LiveDiffCommand {
    Changed,
    Stop,
}

#[derive(Debug)]
pub(crate) enum LiveDiffReload {
    Loaded(HzResult<Changeset>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LiveDiffWatchPath {
    pub(crate) path: PathBuf,
    pub(crate) recursive: bool,
}

impl LiveDiffWatchPath {
    pub(crate) fn recursive_mode(&self) -> RecursiveMode {
        if self.recursive {
            RecursiveMode::Recursive
        } else {
            RecursiveMode::NonRecursive
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct LiveDiffWatchSpec {
    pub(crate) watch_paths: Vec<LiveDiffWatchPath>,
    pub(crate) filter: LiveDiffFilter,
}

impl LiveDiffWatchSpec {
    pub(crate) fn new(repo: &Path) -> Self {
        let mut spec = Self {
            watch_paths: Vec::new(),
            filter: LiveDiffFilter {
                repo: repo.to_path_buf(),
                git_state_paths: Vec::new(),
            },
        };
        spec.add_watch_path(repo.to_path_buf(), true);
        spec
    }

    pub(crate) fn add_git_state_path(&mut self, path: PathBuf) {
        if !self
            .filter
            .git_state_paths
            .iter()
            .any(|known| known == &path)
        {
            self.filter.git_state_paths.push(path);
        }
    }

    pub(crate) fn add_watch_path(&mut self, path: PathBuf, recursive: bool) {
        if let Some(existing) = self
            .watch_paths
            .iter_mut()
            .find(|watch_path| watch_path.path == path)
        {
            existing.recursive |= recursive;
            return;
        }

        self.watch_paths.push(LiveDiffWatchPath { path, recursive });
    }

    pub(crate) fn add_git_state(&mut self, path: PathBuf) {
        self.add_git_state_path(path.clone());
        if path.is_dir() {
            self.add_watch_path(path, true);
        } else if let Some(parent) = path.parent() {
            self.add_watch_path(parent.to_path_buf(), false);
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct LiveDiffFilter {
    pub(crate) repo: PathBuf,
    pub(crate) git_state_paths: Vec<PathBuf>,
}

impl LiveDiffFilter {
    pub(crate) fn is_relevant_event(&self, event: &notify::Event) -> bool {
        if matches!(event.kind, notify::EventKind::Access(_)) {
            return false;
        }

        if event.paths.is_empty() {
            return true;
        }

        event.paths.iter().any(|path| self.is_relevant_path(path))
    }

    pub(crate) fn is_relevant_path(&self, path: &Path) -> bool {
        let joined;
        let path = if path.is_absolute() || path.starts_with(&self.repo) {
            path
        } else {
            joined = self.repo.join(path);
            &joined
        };

        if self.is_git_state_path(path) {
            return true;
        }

        if self.is_inside_repo_dot_git(path) {
            return false;
        }

        path.starts_with(&self.repo)
    }

    pub(crate) fn is_git_state_path(&self, path: &Path) -> bool {
        self.git_state_paths.iter().any(|state_path| {
            path == state_path
                || path.starts_with(state_path)
                || state_path.parent().is_some_and(|parent| path == parent)
        })
    }

    pub(crate) fn is_inside_repo_dot_git(&self, path: &Path) -> bool {
        let Ok(relative) = path.strip_prefix(&self.repo) else {
            return false;
        };

        relative
            .components()
            .next()
            .is_some_and(|component| component.as_os_str() == OsStr::new(".git"))
    }
}

pub(crate) fn live_diff_supported(options: &DiffOptions) -> bool {
    matches!(options.source, DiffSource::Worktree)
}

pub(crate) fn live_diff_watch_spec(repo: &Path) -> HzResult<LiveDiffWatchSpec> {
    let mut spec = LiveDiffWatchSpec::new(repo);
    for git_path in [
        "index",
        "index.lock",
        "HEAD",
        "HEAD.lock",
        "refs",
        "packed-refs",
        "packed-refs.lock",
        "info/exclude",
        "config",
    ] {
        spec.add_git_state(hz_git::git_path(repo, git_path)?);
    }
    Ok(spec)
}

pub(crate) fn spawn_live_diff_worker(
    options: DiffOptions,
    control_rx: Receiver<LiveDiffCommand>,
    reload_tx: Sender<LiveDiffReload>,
    paused: Arc<AtomicBool>,
    pending_while_paused: Arc<AtomicBool>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        while let Ok(LiveDiffCommand::Changed) = control_rx.recv() {
            if !wait_for_live_diff_quiet_period(&control_rx) {
                break;
            }
            if reload_should_wait_for_unpause(&paused, &pending_while_paused) {
                continue;
            }

            let changeset = hz_diff::load_review_ref(&options);
            if reload_should_wait_for_unpause(&paused, &pending_while_paused) {
                continue;
            }
            if reload_tx.send(LiveDiffReload::Loaded(changeset)).is_err() {
                break;
            }
        }
    })
}

fn queue_changed_or_record_pending(
    paused: &AtomicBool,
    pending_while_paused: &AtomicBool,
    control_tx: &Sender<LiveDiffCommand>,
) {
    if paused.load(Ordering::Acquire) {
        pending_while_paused.store(true, Ordering::Release);
        if paused.load(Ordering::Acquire) {
            return;
        }
        if !pending_while_paused.swap(false, Ordering::AcqRel) {
            return;
        }
    }

    let _ = control_tx.send(LiveDiffCommand::Changed);
}

fn flush_pending_paused_reload(
    pending_while_paused: &AtomicBool,
    control_tx: &Sender<LiveDiffCommand>,
) {
    if pending_while_paused.swap(false, Ordering::AcqRel) {
        let _ = control_tx.send(LiveDiffCommand::Changed);
    }
}

fn reload_should_wait_for_unpause(paused: &AtomicBool, pending_while_paused: &AtomicBool) -> bool {
    if !paused.load(Ordering::Acquire) {
        return false;
    }

    pending_while_paused.store(true, Ordering::Release);
    if paused.load(Ordering::Acquire) {
        return true;
    }

    pending_while_paused.swap(false, Ordering::AcqRel);
    false
}

pub(crate) fn wait_for_live_diff_quiet_period(control_rx: &Receiver<LiveDiffCommand>) -> bool {
    loop {
        match control_rx.recv_timeout(LIVE_RELOAD_DEBOUNCE) {
            Ok(LiveDiffCommand::Changed) => continue,
            Ok(LiveDiffCommand::Stop) | Err(RecvTimeoutError::Disconnected) => return false,
            Err(RecvTimeoutError::Timeout) => return true,
        }
    }
}

pub(crate) fn watcher_error(context: &str, error: notify::Error) -> HzError {
    HzError::Usage(format!("{context}: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paused_live_diff_records_and_flushes_pending_reload() {
        let paused = AtomicBool::new(true);
        let pending = AtomicBool::new(false);
        let (tx, rx) = mpsc::channel();

        queue_changed_or_record_pending(&paused, &pending, &tx);

        assert!(pending.load(Ordering::Acquire));
        assert!(matches!(rx.try_recv(), Err(mpsc::TryRecvError::Empty)));

        paused.store(false, Ordering::Release);
        flush_pending_paused_reload(&pending, &tx);

        assert!(!pending.load(Ordering::Acquire));
        assert!(matches!(rx.try_recv(), Ok(LiveDiffCommand::Changed)));
    }

    #[test]
    fn worker_pause_check_marks_pending_reload() {
        let paused = AtomicBool::new(true);
        let pending = AtomicBool::new(false);

        assert!(reload_should_wait_for_unpause(&paused, &pending));
        assert!(pending.load(Ordering::Acquire));
    }
}
