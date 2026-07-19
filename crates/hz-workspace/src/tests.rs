use super::*;
use crate::strategy::TestStrategy;
use tempfile::TempDir;

static CURRENT_DIRECTORY_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn manager(temp: &TempDir) -> Manager {
    Manager::with_strategy(
        temp.path().join("workspaces.sqlite"),
        Box::new(TestStrategy),
    )
    .unwrap()
}

fn root(temp: &TempDir) -> PathBuf {
    let root = temp.path().join("project");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("file.txt"), "root").unwrap();
    fs::canonicalize(root).unwrap()
}

struct FailedCreateCleanupStrategy;

impl Strategy for FailedCreateCleanupStrategy {
    fn name(&self) -> &'static str {
        "failed_create_cleanup"
    }

    fn copy_directory(
        &self,
        _from: &Path,
        to: &Path,
        _mode: CopyMode,
        workspace_id: &str,
    ) -> Result<()> {
        strategy::create_private_directory(to)?;
        marker::write(to, workspace_id)?;
        fs::write(to.join("partial.txt"), "partial")?;
        Err(std::io::Error::other("simulated create failure").into())
    }

    fn initialize_directory(
        &self,
        _path: &Path,
        _progress: &mut dyn FnMut(InitProgress),
    ) -> Result<StrategyInit> {
        Ok(StrategyInit::AlreadyNative)
    }

    fn remove_directory(&self, _path: &Path) -> Result<()> {
        Err(std::io::Error::other("simulated cleanup failure").into())
    }
}

struct InheritedMarkerFailedCreateStrategy;

impl Strategy for InheritedMarkerFailedCreateStrategy {
    fn name(&self) -> &'static str {
        "inherited_marker_failed_create"
    }

    fn copy_directory(
        &self,
        from: &Path,
        to: &Path,
        _mode: CopyMode,
        _workspace_id: &str,
    ) -> Result<()> {
        strategy::create_private_directory(to)?;
        fs::copy(marker::path(from), marker::path(to))?;
        fs::write(to.join("partial.txt"), "partial")?;
        Err(std::io::Error::other("simulated clone failure").into())
    }

    fn initialize_directory(
        &self,
        _path: &Path,
        _progress: &mut dyn FnMut(InitProgress),
    ) -> Result<StrategyInit> {
        Ok(StrategyInit::AlreadyNative)
    }
}

struct FailOnceGcStrategy(std::cell::Cell<bool>);

impl Strategy for FailOnceGcStrategy {
    fn name(&self) -> &'static str {
        "fail_once_gc"
    }

    fn copy_directory(
        &self,
        from: &Path,
        to: &Path,
        mode: CopyMode,
        workspace_id: &str,
    ) -> Result<()> {
        TestStrategy.copy_directory(from, to, mode, workspace_id)
    }

    fn initialize_directory(
        &self,
        _path: &Path,
        _progress: &mut dyn FnMut(InitProgress),
    ) -> Result<StrategyInit> {
        Ok(StrategyInit::AlreadyNative)
    }

    fn remove_directory(&self, path: &Path) -> Result<()> {
        if !self.0.replace(true) {
            marker::remove(path)?;
            return Err(std::io::Error::other("simulated interrupted GC").into());
        }
        strategy::remove_directory_tree(path)
    }
}

#[test]
fn failed_init_setup_does_not_expose_an_active_workspace() {
    let temp = TempDir::new().unwrap();
    let root = root(&temp);
    let mut manager = manager(&temp);

    let result = manager.init_with_setup(
        InitWorkspace {
            at: root.clone(),
            here: true,
            strategy: InitStrategy::CopyOnWrite,
        },
        |_| -> Result<()> { Err(std::io::Error::other("simulated config failure").into()) },
    );

    assert!(matches!(result, Err(Error::Io(_))));
    assert!(manager.registry.all_records().unwrap().is_empty());
    assert_eq!(marker::read(&root).unwrap(), None);
}

#[test]
fn init_adopts_an_existing_unregistered_marker() {
    let temp = TempDir::new().unwrap();
    let root = root(&temp);
    let id = Ulid::new().to_string();
    fs::write(root.join(MARKER_FILE), format!("{id}\n")).unwrap();
    let mut manager = manager(&temp);

    let initialized = manager
        .init(InitWorkspace {
            at: root,
            here: true,
            strategy: InitStrategy::CopyOnWrite,
        })
        .unwrap();

    assert_eq!(initialized.workspace.id, id);
}

#[test]
fn init_rejects_an_orphan_marker_inside_a_managed_workspace() {
    let temp = TempDir::new().unwrap();
    let root = root(&temp);
    let mut manager = manager(&temp);
    manager
        .init(InitWorkspace {
            at: root.clone(),
            here: true,
            strategy: InitStrategy::CopyOnWrite,
        })
        .unwrap();
    let nested = root.join("nested");
    fs::create_dir(&nested).unwrap();
    let orphan_id = Ulid::new().to_string();
    marker::write(&nested, &orphan_id).unwrap();

    assert!(matches!(
        manager.init(InitWorkspace {
            at: nested.clone(),
            here: true,
            strategy: InitStrategy::CopyOnWrite,
        }),
        Err(Error::Path(message)) if message.contains("cannot initialize a nested workspace")
    ));
    assert!(manager.registry.workspace_id(&orphan_id).unwrap().is_none());
    assert_eq!(marker::read(&nested).unwrap(), Some(orphan_id));
}

#[test]
fn init_here_restores_a_missing_registered_marker() {
    let temp = TempDir::new().unwrap();
    let root = root(&temp);
    let mut manager = manager(&temp);
    manager
        .init(InitWorkspace {
            at: root.clone(),
            here: true,
            strategy: InitStrategy::CopyOnWrite,
        })
        .unwrap();
    fs::remove_file(root.join(MARKER_FILE)).unwrap();

    let initialized = manager
        .init(InitWorkspace {
            at: root.clone(),
            here: true,
            strategy: InitStrategy::CopyOnWrite,
        })
        .unwrap();

    assert_eq!(initialized.outcome, InitOutcome::MarkerRestored);
    assert_eq!(marker::read(&root).unwrap(), Some(initialized.workspace.id));
}

#[test]
fn init_without_here_refuses_to_restore_a_missing_marker() {
    let temp = TempDir::new().unwrap();
    let root = root(&temp);
    let mut manager = manager(&temp);
    let initialized = manager
        .init(InitWorkspace {
            at: root.clone(),
            here: true,
            strategy: InitStrategy::CopyOnWrite,
        })
        .unwrap()
        .workspace;
    fs::remove_file(root.join(MARKER_FILE)).unwrap();

    assert!(matches!(
        manager.init(InitWorkspace {
            at: root.clone(),
            here: false,
            strategy: InitStrategy::CopyOnWrite,
        }),
        Err(Error::MissingMarker(path)) if path == initialized.path
    ));
    assert_eq!(marker::read(&root).unwrap(), None);
}

#[test]
fn init_does_not_recover_a_missing_marker_from_an_ancestor() {
    let temp = TempDir::new().unwrap();
    let root = root(&temp);
    let mut manager = manager(&temp);
    let initialized = manager
        .init(InitWorkspace {
            at: root.clone(),
            here: true,
            strategy: InitStrategy::CopyOnWrite,
        })
        .unwrap()
        .workspace;
    fs::remove_dir_all(&root).unwrap();
    let nested = root.join("replacement/nested");
    fs::create_dir_all(&nested).unwrap();
    fs::write(root.join("unrelated.txt"), "keep").unwrap();

    assert!(matches!(
        manager.init(InitWorkspace {
            at: nested.clone(),
            here: false,
            strategy: InitStrategy::CopyOnWrite,
        }),
        Err(Error::MissingMarker(path)) if path == initialized.path
    ));
    assert_eq!(marker::read(&root).unwrap(), None);
    assert_eq!(marker::read(&nested).unwrap(), None);
    assert_eq!(
        fs::read_to_string(root.join("unrelated.txt")).unwrap(),
        "keep"
    );
}

#[test]
fn child_reinitialization_uses_the_family_root_storage() {
    let temp = TempDir::new().unwrap();
    let root = root(&temp);
    let mut manager = manager(&temp);
    manager
        .init(InitWorkspace {
            at: root.clone(),
            here: true,
            strategy: InitStrategy::CopyOnWrite,
        })
        .unwrap();
    let child = manager
        .create(CreateWorkspace {
            from: root,
            handle: Some("child".into()),
            into: None,
            copy_mode: CopyMode::All,
        })
        .unwrap()
        .workspace;

    let initialized = manager
        .init(InitWorkspace {
            at: child.path.clone(),
            here: true,
            strategy: InitStrategy::CopyOnWrite,
        })
        .unwrap();
    assert_eq!(initialized.outcome, InitOutcome::AlreadyInitialized);

    fs::remove_file(child.path.join(MARKER_FILE)).unwrap();
    let recovered = manager
        .init(InitWorkspace {
            at: child.path.clone(),
            here: true,
            strategy: InitStrategy::CopyOnWrite,
        })
        .unwrap();

    assert_eq!(recovered.outcome, InitOutcome::MarkerRestored);
    assert_eq!(recovered.workspace.id, child.id);
    marker::verify(&child.path, &child.id).unwrap();
}

#[test]
fn init_rejects_a_root_containing_a_managed_workspace() {
    let temp = TempDir::new().unwrap();
    let parent = temp.path().join("parent");
    let child = parent.join("child");
    fs::create_dir_all(&child).unwrap();
    let mut manager = manager(&temp);
    manager
        .init(InitWorkspace {
            at: child.clone(),
            here: true,
            strategy: InitStrategy::CopyOnWrite,
        })
        .unwrap();

    assert!(matches!(
        manager.init(InitWorkspace {
            at: parent.clone(),
            here: true,
            strategy: InitStrategy::CopyOnWrite,
        }),
        Err(Error::ContainsManagedWorkspace(path)) if path == fs::canonicalize(child).unwrap()
    ));
    assert!(!parent.join(MARKER_FILE).exists());
}

#[test]
fn workspace_lifecycle_does_not_infer_source_control_roots() {
    let temp = TempDir::new().unwrap();
    let root = root(&temp);
    fs::create_dir(root.join(".git")).unwrap();
    let nested = root.join("packages/app");
    fs::create_dir_all(&nested).unwrap();
    let mut manager = manager(&temp);

    let initialized = manager
        .init(InitWorkspace {
            at: nested.clone(),
            here: false,
            strategy: InitStrategy::CopyOnWrite,
        })
        .unwrap();

    assert_eq!(
        initialized.workspace.path,
        fs::canonicalize(nested).unwrap()
    );
    assert!(!root.join(MARKER_FILE).exists());
    assert!(
        fs::read_to_string(root.join(".git/info/exclude"))
            .unwrap()
            .lines()
            .any(|line| line == MARKER_FILE)
    );
}

#[test]
fn init_excludes_the_marker_from_normal_git_adds() {
    let temp = TempDir::new().unwrap();
    let root = root(&temp);
    let output = std::process::Command::new("git")
        .args(["init", "-q"])
        .arg(&root)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git init failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut manager = manager(&temp);

    manager
        .init(InitWorkspace {
            at: root.clone(),
            here: true,
            strategy: InitStrategy::CopyOnWrite,
        })
        .unwrap();

    let ignored = std::process::Command::new("git")
        .arg("-C")
        .arg(&root)
        .args(["check-ignore", "--quiet", "--", MARKER_FILE])
        .status()
        .unwrap();
    assert!(ignored.success());
    let added = std::process::Command::new("git")
        .arg("-C")
        .arg(&root)
        .args(["add", "-A"])
        .status()
        .unwrap();
    assert!(added.success());
    let tracked = std::process::Command::new("git")
        .arg("-C")
        .arg(&root)
        .args(["ls-files", "--", MARKER_FILE])
        .output()
        .unwrap();
    assert!(tracked.status.success());
    assert!(tracked.stdout.is_empty());
}

#[test]
fn default_storage_is_ignored_by_an_enclosing_git_checkout() {
    let temp = TempDir::new().unwrap();
    let repository = temp.path().join("repository");
    let root = repository.join("packages/project");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("file.txt"), "root").unwrap();
    let output = std::process::Command::new("git")
        .args(["init", "-q"])
        .arg(&repository)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git init failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let repository = fs::canonicalize(repository).unwrap();
    let root = fs::canonicalize(root).unwrap();
    let mut manager = manager(&temp);
    manager
        .init(InitWorkspace {
            at: root.clone(),
            here: true,
            strategy: InitStrategy::CopyOnWrite,
        })
        .unwrap();

    let child = manager
        .create(CreateWorkspace {
            from: root,
            handle: Some("child".into()),
            into: None,
            copy_mode: CopyMode::All,
        })
        .unwrap()
        .workspace;
    let relative_child = child.path.strip_prefix(&repository).unwrap();

    let ignored = std::process::Command::new("git")
        .arg("-C")
        .arg(&repository)
        .args(["check-ignore", "--quiet", "--"])
        .arg(relative_child)
        .status()
        .unwrap();
    assert!(ignored.success());
    let added = std::process::Command::new("git")
        .arg("-C")
        .arg(&repository)
        .args(["add", "-A"])
        .status()
        .unwrap();
    assert!(added.success());
    let tracked = std::process::Command::new("git")
        .arg("-C")
        .arg(&repository)
        .args(["ls-files", "--", "packages/.hz-workspaces"])
        .output()
        .unwrap();
    assert!(tracked.status.success());
    assert!(tracked.stdout.is_empty());
}

#[test]
fn init_configures_a_repository_local_mercurial_ignore() {
    let temp = TempDir::new().unwrap();
    let root = root(&temp);
    fs::create_dir(root.join(".hg")).unwrap();
    let mut manager = manager(&temp);

    manager
        .init(InitWorkspace {
            at: root.clone(),
            here: true,
            strategy: InitStrategy::CopyOnWrite,
        })
        .unwrap();

    assert_eq!(
        fs::read_to_string(root.join(".hg/hz-workspace.ignore")).unwrap(),
        "syntax: regexp\n\
         (?:^|/)\\.hz-workspace$\n\
         (?:^|/)\\.hz-workspaces(?:/|$)\n"
    );
    assert!(
        fs::read_to_string(root.join(".hg/hgrc"))
            .unwrap()
            .contains("ignore.hz-workspace = .hg/hz-workspace.ignore")
    );
}

#[test]
fn source_control_metadata_is_copied_without_rewriting_existing_state() {
    let temp = TempDir::new().unwrap();
    let root = root(&temp);
    fs::create_dir(root.join(".git")).unwrap();
    fs::write(root.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
    let mut manager = manager(&temp);
    manager
        .init(InitWorkspace {
            at: root.clone(),
            here: true,
            strategy: InitStrategy::CopyOnWrite,
        })
        .unwrap();

    let child = manager
        .create(CreateWorkspace {
            from: root,
            handle: Some("child".into()),
            into: None,
            copy_mode: CopyMode::All,
        })
        .unwrap();

    assert_eq!(
        fs::read_to_string(child.workspace.path.join(".git/HEAD")).unwrap(),
        "ref: refs/heads/main\n"
    );
}

#[test]
fn explicit_copy_strategy_is_inherited_by_children() {
    let temp = TempDir::new().unwrap();
    let root = root(&temp);
    let mut manager = manager(&temp);
    let initialized = manager
        .init(InitWorkspace {
            at: root.clone(),
            here: true,
            strategy: InitStrategy::Copy,
        })
        .unwrap();
    let child = manager
        .create(CreateWorkspace {
            from: root,
            handle: Some("child".into()),
            into: None,
            copy_mode: CopyMode::All,
        })
        .unwrap();

    assert_eq!(initialized.workspace.strategy, "copy");
    assert_eq!(child.workspace.strategy, "copy");
}

#[test]
fn failed_create_cleanup_keeps_the_recovery_record() {
    let temp = TempDir::new().unwrap();
    let root = root(&temp);
    let mut manager = Manager::with_strategy(
        temp.path().join("workspaces.sqlite"),
        Box::new(FailedCreateCleanupStrategy),
    )
    .unwrap();
    manager
        .init(InitWorkspace {
            at: root.clone(),
            here: true,
            strategy: InitStrategy::CopyOnWrite,
        })
        .unwrap();

    assert!(matches!(
        manager.create(CreateWorkspace {
            from: root,
            handle: Some("child".into()),
            into: None,
            copy_mode: CopyMode::All,
        }),
        Err(Error::Io(_))
    ));

    let creating = manager
        .registry
        .all_records()
        .unwrap()
        .into_iter()
        .find(|record| record.state == WorkspaceState::Creating)
        .expect("failed cleanup should retain the creating row");
    let staging = manager
        .registry
        .staging_path(&creating.id)
        .unwrap()
        .expect("creating row should retain its staging path");
    assert!(staging.exists());
    marker::verify(&staging, &creating.id).unwrap();
    assert!(
        manager
            .doctor(false)
            .unwrap()
            .issues
            .iter()
            .any(|issue| issue.workspace_id == creating.id)
    );
}

#[test]
fn failed_create_cleanup_removes_an_unfiltered_clone_with_the_source_marker() {
    let temp = TempDir::new().unwrap();
    let root = root(&temp);
    let mut manager = Manager::with_strategy(
        temp.path().join("workspaces.sqlite"),
        Box::new(InheritedMarkerFailedCreateStrategy),
    )
    .unwrap();
    let initialized = manager
        .init(InitWorkspace {
            at: root.clone(),
            here: true,
            strategy: InitStrategy::CopyOnWrite,
        })
        .unwrap()
        .workspace;
    let storage = initialized.storage_path.unwrap();

    assert!(matches!(
        manager.create(CreateWorkspace {
            from: root,
            handle: Some("child".into()),
            into: None,
            copy_mode: CopyMode::All,
        }),
        Err(Error::Io(_))
    ));

    assert!(
        manager
            .registry
            .all_records()
            .unwrap()
            .iter()
            .all(|record| record.state != WorkspaceState::Creating)
    );
    assert!(fs::read_dir(storage).unwrap().next().is_none());
}

#[test]
fn creates_a_logical_tree_in_flat_family_storage() {
    let temp = TempDir::new().unwrap();
    let root = root(&temp);
    let mut manager = manager(&temp);
    let initialized = manager
        .init(InitWorkspace {
            at: root.clone(),
            here: true,
            strategy: InitStrategy::CopyOnWrite,
        })
        .unwrap();
    let first = manager
        .create(CreateWorkspace {
            from: root.clone(),
            handle: Some("first".into()),
            into: None,
            copy_mode: CopyMode::All,
        })
        .unwrap();
    let second = manager
        .create(CreateWorkspace {
            from: first.workspace.path.clone(),
            handle: Some("second".into()),
            into: None,
            copy_mode: CopyMode::All,
        })
        .unwrap();

    assert_eq!(
        first.workspace.parent_id.as_deref(),
        Some(initialized.workspace.id.as_str())
    );
    assert_eq!(
        second.workspace.parent_id.as_deref(),
        Some(first.workspace.id.as_str())
    );
    assert_eq!(
        first.workspace.path.parent(),
        second.workspace.path.parent()
    );
    assert_eq!(
        manager
            .ancestors(&second.workspace.path, None)
            .unwrap()
            .into_iter()
            .map(|workspace| workspace.handle)
            .collect::<Vec<_>>(),
        vec!["first", "project"]
    );
}

#[test]
fn custom_storage_cannot_be_nested_inside_another_workspace() {
    let temp = TempDir::new().unwrap();
    let root = root(&temp);
    let mut manager = manager(&temp);
    manager
        .init(InitWorkspace {
            at: root.clone(),
            here: true,
            strategy: InitStrategy::CopyOnWrite,
        })
        .unwrap();
    let first = manager
        .create(CreateWorkspace {
            from: root.clone(),
            handle: Some("first".into()),
            into: None,
            copy_mode: CopyMode::All,
        })
        .unwrap();

    assert!(matches!(
        manager.create(CreateWorkspace {
            from: root,
            handle: Some("nested".into()),
            into: Some(first.workspace.path.join("storage")),
            copy_mode: CopyMode::All,
        }),
        Err(Error::InsideManagedWorkspace(_))
    ));
}

#[test]
fn filtered_creation_omits_regenerable_artifacts() {
    let temp = TempDir::new().unwrap();
    let root = root(&temp);
    fs::create_dir_all(root.join("node_modules/pkg")).unwrap();
    fs::write(root.join("node_modules/pkg/index.js"), "module").unwrap();
    let mut manager = manager(&temp);
    manager
        .init(InitWorkspace {
            at: root.clone(),
            here: true,
            strategy: InitStrategy::CopyOnWrite,
        })
        .unwrap();

    let child = manager
        .create(CreateWorkspace {
            from: root,
            handle: Some("child".into()),
            into: None,
            copy_mode: CopyMode::Filtered,
        })
        .unwrap();

    assert!(!child.workspace.path.join("node_modules").exists());
    assert_eq!(
        fs::read_to_string(child.workspace.path.join("file.txt")).unwrap(),
        "root"
    );
}

#[test]
fn removal_trashes_and_restores_a_subtree() {
    let temp = TempDir::new().unwrap();
    let root = root(&temp);
    let mut manager = manager(&temp);
    manager
        .init(InitWorkspace {
            at: root.clone(),
            here: true,
            strategy: InitStrategy::CopyOnWrite,
        })
        .unwrap();
    let first = manager
        .create(CreateWorkspace {
            from: root.clone(),
            handle: Some("first".into()),
            into: None,
            copy_mode: CopyMode::All,
        })
        .unwrap();
    let second = manager
        .create(CreateWorkspace {
            from: first.workspace.path.clone(),
            handle: Some("second".into()),
            into: None,
            copy_mode: CopyMode::All,
        })
        .unwrap();
    let first_path = first.workspace.path.clone();
    let second_path = second.workspace.path.clone();

    let removed = manager
        .remove(&root, Some("first"), RemoveMode::Subtree, false)
        .unwrap();
    assert_eq!(removed.removed.len(), 2);
    assert_eq!(removed.selected.state, WorkspaceState::Trashed);
    assert_eq!(removed.selected.original_path.as_ref(), Some(&first_path));
    assert_ne!(removed.selected.path, first_path);
    assert!(removed.removed.iter().all(|workspace| {
        workspace.state == WorkspaceState::Trashed && workspace.original_path.is_some()
    }));
    assert!(!first_path.exists());
    assert!(!second_path.exists());

    let restored = manager
        .restore(&root, first_path.to_str().unwrap())
        .unwrap();
    assert_eq!(restored.len(), 2);
    assert!(first_path.exists());
    assert!(second_path.exists());
}

#[test]
fn restoring_a_parent_does_not_restore_an_independently_trashed_child() {
    let temp = TempDir::new().unwrap();
    let root = root(&temp);
    let mut manager = manager(&temp);
    manager
        .init(InitWorkspace {
            at: root.clone(),
            here: true,
            strategy: InitStrategy::CopyOnWrite,
        })
        .unwrap();
    let parent = manager
        .create(CreateWorkspace {
            from: root.clone(),
            handle: Some("parent".into()),
            into: None,
            copy_mode: CopyMode::All,
        })
        .unwrap()
        .workspace;
    let child = manager
        .create(CreateWorkspace {
            from: parent.path.clone(),
            handle: Some("child".into()),
            into: None,
            copy_mode: CopyMode::All,
        })
        .unwrap()
        .workspace;

    manager
        .remove(&root, Some("child"), RemoveMode::Subtree, false)
        .unwrap();
    manager
        .remove(&root, Some("parent"), RemoveMode::Subtree, false)
        .unwrap();

    let restored = manager.restore(&root, "parent").unwrap();

    assert_eq!(restored.len(), 1);
    assert_eq!(restored[0].id, parent.id);
    assert!(parent.path.exists());
    assert!(!child.path.exists());
    assert_eq!(
        manager
            .registry
            .workspace_id(&child.id)
            .unwrap()
            .unwrap()
            .state,
        WorkspaceState::Trashed
    );
}

#[test]
fn trashed_workspace_is_a_valid_restore_and_listing_context() {
    let temp = TempDir::new().unwrap();
    let root = root(&temp);
    let mut manager = manager(&temp);
    manager
        .init(InitWorkspace {
            at: root.clone(),
            here: true,
            strategy: InitStrategy::CopyOnWrite,
        })
        .unwrap();
    let child = manager
        .create(CreateWorkspace {
            from: root,
            handle: Some("child".into()),
            into: None,
            copy_mode: CopyMode::All,
        })
        .unwrap()
        .workspace;
    fs::create_dir(child.path.join("nested")).unwrap();

    let removed = manager
        .remove(child.path.join("nested"), None, RemoveMode::Subtree, false)
        .unwrap();
    let trash_context = removed.selected.path.join("nested");

    let targets = manager.trashed(&trash_context).unwrap();
    assert!(targets.iter().any(|workspace| workspace.id == child.id));

    let restored = manager.restore(&trash_context, "child").unwrap();
    assert!(restored.iter().any(|workspace| workspace.id == child.id));
    assert!(child.path.exists());
}

#[cfg(unix)]
#[test]
fn removal_rejects_a_symlinked_active_workspace() {
    use std::os::unix::fs::symlink;

    let temp = TempDir::new().unwrap();
    let root = root(&temp);
    let external = temp.path().join("external");
    let mut manager = manager(&temp);
    manager
        .init(InitWorkspace {
            at: root.clone(),
            here: true,
            strategy: InitStrategy::CopyOnWrite,
        })
        .unwrap();
    let child = manager
        .create(CreateWorkspace {
            from: root.clone(),
            handle: Some("child".into()),
            into: None,
            copy_mode: CopyMode::All,
        })
        .unwrap()
        .workspace;
    fs::remove_dir_all(&child.path).unwrap();
    fs::create_dir(&external).unwrap();
    let external = fs::canonicalize(external).unwrap();
    fs::write(external.join("outside.txt"), "keep").unwrap();
    marker::write(&external, &child.id).unwrap();
    symlink(&external, &child.path).unwrap();

    assert!(matches!(
        manager.remove(&root, Some("child"), RemoveMode::Subtree, false),
        Err(Error::InvalidMarker(path)) if path == child.path
    ));
    assert_eq!(
        fs::read_to_string(external.join("outside.txt")).unwrap(),
        "keep"
    );
    marker::verify(&external, &child.id).unwrap();
    assert_eq!(
        manager
            .registry
            .workspace_id(&child.id)
            .unwrap()
            .unwrap()
            .state,
        WorkspaceState::Active
    );
}

#[cfg(unix)]
#[test]
fn removal_rejects_a_symlinked_trash_directory() {
    use std::os::unix::fs::symlink;

    let temp = TempDir::new().unwrap();
    let root = root(&temp);
    let external = temp.path().join("external-trash");
    let mut manager = manager(&temp);
    manager
        .init(InitWorkspace {
            at: root.clone(),
            here: true,
            strategy: InitStrategy::CopyOnWrite,
        })
        .unwrap();
    let child = manager
        .create(CreateWorkspace {
            from: root.clone(),
            handle: Some("child".into()),
            into: None,
            copy_mode: CopyMode::All,
        })
        .unwrap()
        .workspace;
    fs::create_dir(&external).unwrap();
    let external = fs::canonicalize(external).unwrap();
    let trash = child.path.parent().unwrap().join(".trash");
    symlink(&external, &trash).unwrap();

    assert!(matches!(
        manager.remove(&root, Some("child"), RemoveMode::Subtree, false),
        Err(Error::InvalidMarker(path)) if path == trash
    ));
    assert!(child.path.exists());
    assert!(!external.join(&child.id).exists());
    marker::verify(&child.path, &child.id).unwrap();
    assert_eq!(
        manager
            .registry
            .workspace_id(&child.id)
            .unwrap()
            .unwrap()
            .state,
        WorkspaceState::Active
    );
}

#[test]
fn restore_rejects_a_destination_inside_another_managed_workspace() {
    let temp = TempDir::new().unwrap();
    let root = root(&temp);
    let mut manager = manager(&temp);
    manager
        .init(InitWorkspace {
            at: root.clone(),
            here: true,
            strategy: InitStrategy::CopyOnWrite,
        })
        .unwrap();
    let child = manager
        .create(CreateWorkspace {
            from: root.clone(),
            handle: Some("child".into()),
            into: None,
            copy_mode: CopyMode::All,
        })
        .unwrap()
        .workspace;
    manager
        .remove(&root, Some("child"), RemoveMode::Subtree, false)
        .unwrap();
    let destination_parent = child.path.parent().unwrap().to_path_buf();
    manager
        .init(InitWorkspace {
            at: destination_parent,
            here: true,
            strategy: InitStrategy::CopyOnWrite,
        })
        .unwrap();

    assert!(matches!(
        manager.restore(&root, child.path.to_str().unwrap()),
        Err(Error::InsideManagedWorkspace(path)) if path == child.path
    ));
    let still_trashed = manager.registry.workspace_id(&child.id).unwrap().unwrap();
    assert_eq!(still_trashed.state, WorkspaceState::Trashed);
    assert!(still_trashed.path.exists());
    assert!(!child.path.exists());
}

#[test]
fn restore_rejects_an_active_workspace_inside_an_unregistered_root() {
    let temp = TempDir::new().unwrap();
    let root = root(&temp);
    let source = temp.path().join("source");
    fs::create_dir(&source).unwrap();
    fs::write(source.join("file.txt"), "source").unwrap();
    let mut manager = manager(&temp);
    let removed_root = manager
        .init(InitWorkspace {
            at: root.clone(),
            here: true,
            strategy: InitStrategy::CopyOnWrite,
        })
        .unwrap()
        .workspace;
    manager
        .remove(&root, None, RemoveMode::Subtree, true)
        .unwrap();
    manager
        .init(InitWorkspace {
            at: source.clone(),
            here: true,
            strategy: InitStrategy::CopyOnWrite,
        })
        .unwrap();
    let foreign = manager
        .create(CreateWorkspace {
            from: source,
            handle: Some("foreign".into()),
            into: Some(root.clone()),
            copy_mode: CopyMode::All,
        })
        .unwrap()
        .workspace;

    assert!(matches!(
        manager.restore(&root, root.to_str().unwrap()),
        Err(Error::ContainsManagedWorkspace(path)) if path == foreign.path
    ));
    assert_eq!(marker::read(&root).unwrap(), None);
    assert_eq!(
        manager
            .registry
            .workspace_id(&removed_root.id)
            .unwrap()
            .unwrap()
            .state,
        WorkspaceState::Unregistered
    );
    marker::verify(&foreign.path, &foreign.id).unwrap();
}

#[cfg(unix)]
#[test]
fn restore_checks_the_resolved_ancestry_of_a_symlinked_parent() {
    use std::os::unix::fs::symlink;

    let temp = TempDir::new().unwrap();
    let root = root(&temp);
    let host = temp.path().join("host");
    let host_nested = host.join("nested");
    fs::create_dir_all(&host_nested).unwrap();
    let destination_parent = temp.path().join("restore-parent");
    fs::create_dir(&destination_parent).unwrap();
    let mut manager = manager(&temp);
    manager
        .init(InitWorkspace {
            at: root.clone(),
            here: true,
            strategy: InitStrategy::CopyOnWrite,
        })
        .unwrap();
    manager
        .init(InitWorkspace {
            at: host.clone(),
            here: true,
            strategy: InitStrategy::CopyOnWrite,
        })
        .unwrap();
    let child = manager
        .create(CreateWorkspace {
            from: root.clone(),
            handle: Some("child".into()),
            into: Some(destination_parent.clone()),
            copy_mode: CopyMode::All,
        })
        .unwrap()
        .workspace;
    manager
        .remove(&root, Some("child"), RemoveMode::Subtree, false)
        .unwrap();

    let trashed = manager.registry.workspace_id(&child.id).unwrap().unwrap();
    let relocated_trash = temp.path().join("relocated-trash");
    fs::rename(&trashed.path, &relocated_trash).unwrap();
    let relocated_trash = fs::canonicalize(relocated_trash).unwrap();
    manager
        .registry
        .update_path(&child.id, &relocated_trash)
        .unwrap();
    fs::remove_dir_all(&destination_parent).unwrap();
    symlink(&host_nested, &destination_parent).unwrap();

    assert!(matches!(
        manager.restore(&root, "child"),
        Err(Error::InsideManagedWorkspace(path)) if path == child.path
    ));
    assert!(relocated_trash.exists());
    assert!(!host_nested.join(&child.id).exists());
    assert_eq!(
        manager
            .registry
            .workspace_id(&child.id)
            .unwrap()
            .unwrap()
            .state,
        WorkspaceState::Trashed
    );
}

#[cfg(unix)]
#[test]
fn restore_rejects_changed_symlink_ancestry_even_when_the_target_is_unmanaged() {
    use std::os::unix::fs::symlink;

    let temp = TempDir::new().unwrap();
    let root = root(&temp);
    let destination_parent = temp.path().join("restore-parent");
    let redirected_parent = temp.path().join("redirected-parent");
    fs::create_dir(&destination_parent).unwrap();
    fs::create_dir(&redirected_parent).unwrap();
    let mut manager = manager(&temp);
    manager
        .init(InitWorkspace {
            at: root.clone(),
            here: true,
            strategy: InitStrategy::CopyOnWrite,
        })
        .unwrap();
    let child = manager
        .create(CreateWorkspace {
            from: root.clone(),
            handle: Some("child".into()),
            into: Some(destination_parent.clone()),
            copy_mode: CopyMode::All,
        })
        .unwrap()
        .workspace;
    manager
        .remove(&root, Some("child"), RemoveMode::Subtree, false)
        .unwrap();
    let mut trashed = manager.registry.workspace_id(&child.id).unwrap().unwrap();
    let relocated_trash = temp.path().join("relocated-trash");
    fs::rename(&trashed.path, &relocated_trash).unwrap();
    let relocated_trash = fs::canonicalize(relocated_trash).unwrap();
    manager
        .registry
        .update_path(&child.id, &relocated_trash)
        .unwrap();
    trashed.path = relocated_trash;

    fs::remove_dir_all(&destination_parent).unwrap();
    symlink(&redirected_parent, &destination_parent).unwrap();

    assert!(matches!(
        manager.restore(&root, "child"),
        Err(Error::Path(message)) if message.contains("restore destination ancestry changed")
    ));
    assert!(trashed.path.exists());
    assert!(!redirected_parent.join(&child.id).exists());
    let still_trashed = manager.registry.workspace_id(&child.id).unwrap().unwrap();
    assert_eq!(still_trashed.state, WorkspaceState::Trashed);
    assert_eq!(still_trashed.path, trashed.path);
}

#[test]
fn doctor_does_not_overwrite_a_mismatched_marker() {
    let temp = TempDir::new().unwrap();
    let root = root(&temp);
    let mut manager = manager(&temp);
    manager
        .init(InitWorkspace {
            at: root.clone(),
            here: true,
            strategy: InitStrategy::CopyOnWrite,
        })
        .unwrap();
    let other = Ulid::new().to_string();
    fs::write(root.join(MARKER_FILE), format!("{other}\n")).unwrap();

    let report = manager.doctor(true).unwrap();

    assert_eq!(report.issues.len(), 1);
    assert!(!report.issues[0].fixed);
    assert_eq!(
        fs::read_to_string(root.join(MARKER_FILE)).unwrap(),
        format!("{other}\n")
    );
}

#[test]
fn doctor_does_not_claim_a_replacement_directory_with_a_missing_marker() {
    let temp = TempDir::new().unwrap();
    let root = root(&temp);
    let mut manager = manager(&temp);
    let initialized = manager
        .init(InitWorkspace {
            at: root.clone(),
            here: true,
            strategy: InitStrategy::CopyOnWrite,
        })
        .unwrap()
        .workspace;
    fs::remove_dir_all(&root).unwrap();
    fs::create_dir(&root).unwrap();
    fs::write(root.join("unrelated.txt"), "keep").unwrap();

    let report = manager.doctor(true).unwrap();

    let issue = report
        .issues
        .iter()
        .find(|issue| issue.workspace_id == initialized.id)
        .unwrap();
    assert_eq!(issue.message, "workspace marker is missing");
    assert!(!issue.fixed);
    assert!(!root.join(MARKER_FILE).exists());
    assert_eq!(
        fs::read_to_string(root.join("unrelated.txt")).unwrap(),
        "keep"
    );
}

#[test]
fn concurrent_creates_are_serialized_across_managers() {
    let temp = TempDir::new().unwrap();
    let root = root(&temp);
    let mut initialized = manager(&temp);
    initialized
        .init(InitWorkspace {
            at: root.clone(),
            here: true,
            strategy: InitStrategy::CopyOnWrite,
        })
        .unwrap();
    drop(initialized);

    let database = temp.path().join("workspaces.sqlite");
    let handles = ["one", "two"];
    let threads = handles.map(|handle| {
        let database = database.clone();
        let root = root.clone();
        std::thread::spawn(move || {
            let mut manager = Manager::with_strategy(database, Box::new(TestStrategy)).unwrap();
            manager
                .create(CreateWorkspace {
                    from: root,
                    handle: Some(handle.into()),
                    into: None,
                    copy_mode: CopyMode::All,
                })
                .unwrap()
        })
    });
    for thread in threads {
        thread.join().unwrap();
    }

    let manager = manager(&temp);
    assert_eq!(
        manager
            .list(ListWorkspaces {
                of: Some(root),
                scope: ListScope::Family,
                pinned: None,
            })
            .unwrap()
            .len(),
        3
    );
}

#[test]
fn doctor_recovers_an_interrupted_removal() {
    let temp = TempDir::new().unwrap();
    let root = root(&temp);
    let mut manager = manager(&temp);
    manager
        .init(InitWorkspace {
            at: root.clone(),
            here: true,
            strategy: InitStrategy::CopyOnWrite,
        })
        .unwrap();
    let child = manager
        .create(CreateWorkspace {
            from: root,
            handle: Some("child".into()),
            into: None,
            copy_mode: CopyMode::All,
        })
        .unwrap();
    let trash = trash_path(&child.workspace.path, &child.workspace.id).unwrap();
    fs::create_dir_all(trash.parent().unwrap()).unwrap();
    fs::rename(&child.workspace.path, &trash).unwrap();

    let report = manager.doctor(true).unwrap();

    assert_eq!(report.issues.len(), 1);
    assert!(report.issues[0].fixed);
    assert_eq!(
        manager
            .registry
            .workspace_id(&child.workspace.id)
            .unwrap()
            .unwrap()
            .state,
        WorkspaceState::Trashed
    );
}

#[test]
fn adopts_a_workspace_after_an_external_move() {
    let temp = TempDir::new().unwrap();
    let root = root(&temp);
    let mut manager = manager(&temp);
    manager
        .init(InitWorkspace {
            at: root.clone(),
            here: true,
            strategy: InitStrategy::CopyOnWrite,
        })
        .unwrap();
    let child = manager
        .create(CreateWorkspace {
            from: root,
            handle: Some("child".into()),
            into: None,
            copy_mode: CopyMode::All,
        })
        .unwrap();
    let moved = temp.path().join("moved-child");
    fs::rename(&child.workspace.path, &moved).unwrap();

    let adopted = manager.adopt(&moved).unwrap();

    assert_eq!(adopted.path, fs::canonicalize(moved).unwrap());
    assert_eq!(adopted.id, child.workspace.id);
}

#[test]
fn trash_targets_include_an_unregistered_root() {
    let temp = TempDir::new().unwrap();
    let root = root(&temp);
    let mut manager = manager(&temp);
    let initialized = manager
        .init(InitWorkspace {
            at: root.clone(),
            here: true,
            strategy: InitStrategy::CopyOnWrite,
        })
        .unwrap()
        .workspace;
    let child = manager
        .create(CreateWorkspace {
            from: root.clone(),
            handle: Some("child".into()),
            into: None,
            copy_mode: CopyMode::All,
        })
        .unwrap()
        .workspace;
    manager
        .remove(&root, None, RemoveMode::Subtree, true)
        .unwrap();

    let targets = manager.trashed(&root).unwrap();

    assert_eq!(targets.len(), 2);
    assert!(targets.iter().any(|workspace| {
        workspace.id == initialized.id && workspace.state == WorkspaceState::Unregistered
    }));
    assert!(targets.iter().any(|workspace| {
        workspace.id == child.id && workspace.state == WorkspaceState::Trashed
    }));
}

#[test]
fn unregistered_root_and_descendants_can_be_restored_by_root_path() {
    let temp = TempDir::new().unwrap();
    let root = root(&temp);
    let mut manager = manager(&temp);
    manager
        .init(InitWorkspace {
            at: root.clone(),
            here: true,
            strategy: InitStrategy::CopyOnWrite,
        })
        .unwrap();
    let child = manager
        .create(CreateWorkspace {
            from: root.clone(),
            handle: Some("child".into()),
            into: None,
            copy_mode: CopyMode::All,
        })
        .unwrap();

    let removed = manager
        .remove(&root, None, RemoveMode::Subtree, true)
        .unwrap();
    assert_eq!(removed.selected.state, WorkspaceState::Unregistered);
    assert_eq!(removed.removed.len(), 1);
    assert_eq!(removed.removed[0].state, WorkspaceState::Trashed);
    assert_eq!(
        removed.removed[0].original_path.as_ref(),
        Some(&child.workspace.path)
    );
    let restored = manager.restore(&root, root.to_str().unwrap()).unwrap();

    assert_eq!(restored.len(), 2);
    assert!(root.join(MARKER_FILE).exists());
    assert!(child.workspace.path.exists());
}

#[test]
fn unregistered_root_can_be_listed_and_restored_from_a_nested_path() {
    let temp = TempDir::new().unwrap();
    let root = root(&temp);
    let nested = root.join("nested");
    fs::create_dir(&nested).unwrap();
    let mut manager = manager(&temp);
    let initialized = manager
        .init(InitWorkspace {
            at: root.clone(),
            here: true,
            strategy: InitStrategy::CopyOnWrite,
        })
        .unwrap()
        .workspace;
    manager
        .remove(&root, None, RemoveMode::Subtree, true)
        .unwrap();

    let trashed = manager.trashed(&nested).unwrap();
    assert!(
        trashed
            .iter()
            .any(|workspace| workspace.id == initialized.id)
    );

    let restored = manager.restore(&nested, "root").unwrap();
    assert_eq!(restored.len(), 1);
    assert_eq!(restored[0].id, initialized.id);
    assert!(root.join(MARKER_FILE).exists());
}

#[test]
fn restore_rejects_a_foreign_marker_on_a_reused_unregistered_root() {
    let temp = TempDir::new().unwrap();
    let root = root(&temp);
    let mut manager = manager(&temp);
    let initialized = manager
        .init(InitWorkspace {
            at: root.clone(),
            here: true,
            strategy: InitStrategy::CopyOnWrite,
        })
        .unwrap()
        .workspace;
    manager
        .remove(&root, None, RemoveMode::Subtree, true)
        .unwrap();
    let foreign_root = temp.path().join("foreign");
    fs::create_dir(&foreign_root).unwrap();
    let foreign_id = manager
        .init(InitWorkspace {
            at: foreign_root,
            here: true,
            strategy: InitStrategy::CopyOnWrite,
        })
        .unwrap()
        .workspace
        .id;
    marker::write(&root, &foreign_id).unwrap();

    assert!(matches!(
        manager.restore(&root, root.to_str().unwrap()),
        Err(Error::MarkerMismatch(path)) if path == root
    ));
    assert_eq!(
        marker::read(&root).unwrap().as_deref(),
        Some(foreign_id.as_str())
    );
    assert_eq!(
        manager
            .registry
            .workspace_id(&initialized.id)
            .unwrap()
            .unwrap()
            .state,
        WorkspaceState::Unregistered
    );
}

#[test]
fn failed_restore_preserves_a_matching_preexisting_root_marker() {
    let temp = TempDir::new().unwrap();
    let root = root(&temp);
    let database = temp.path().join("workspaces.sqlite");
    let mut manager = manager(&temp);
    let initialized = manager
        .init(InitWorkspace {
            at: root.clone(),
            here: true,
            strategy: InitStrategy::CopyOnWrite,
        })
        .unwrap()
        .workspace;
    manager
        .remove(&root, None, RemoveMode::Subtree, true)
        .unwrap();
    marker::write(&root, &initialized.id).unwrap();
    rusqlite::Connection::open(database)
        .unwrap()
        .execute_batch(
            "CREATE TRIGGER fail_test_restore
             BEFORE UPDATE OF state ON workspace
             WHEN OLD.state = 'unregistered' AND NEW.state = 'active'
             BEGIN SELECT RAISE(FAIL, 'simulated restore failure'); END;",
        )
        .unwrap();

    assert!(matches!(
        manager.restore(&root, root.to_str().unwrap()),
        Err(Error::Database(_))
    ));
    marker::verify(&root, &initialized.id).unwrap();
    assert_eq!(
        manager
            .registry
            .workspace_id(&initialized.id)
            .unwrap()
            .unwrap()
            .state,
        WorkspaceState::Unregistered
    );
}

#[test]
fn doctor_completes_an_interrupted_root_family_restore_atomically() {
    let temp = TempDir::new().unwrap();
    let root = root(&temp);
    let mut manager = manager(&temp);
    let initialized = manager
        .init(InitWorkspace {
            at: root.clone(),
            here: true,
            strategy: InitStrategy::CopyOnWrite,
        })
        .unwrap()
        .workspace;
    let first = manager
        .create(CreateWorkspace {
            from: root.clone(),
            handle: Some("first".into()),
            into: None,
            copy_mode: CopyMode::All,
        })
        .unwrap()
        .workspace;
    let second = manager
        .create(CreateWorkspace {
            from: root.clone(),
            handle: Some("second".into()),
            into: None,
            copy_mode: CopyMode::All,
        })
        .unwrap()
        .workspace;
    manager
        .remove(&root, None, RemoveMode::Subtree, true)
        .unwrap();

    marker::write(&root, &initialized.id).unwrap();
    let trashed_first = manager.registry.workspace_id(&first.id).unwrap().unwrap();
    fs::rename(
        &trashed_first.path,
        trashed_first.original_path.as_ref().unwrap(),
    )
    .unwrap();

    let report = manager.doctor(true).unwrap();

    assert!(report.issues.iter().any(|issue| {
        issue.workspace_id == initialized.id
            && issue.message == "root-family workspace restore was interrupted"
            && issue.fixed
    }));
    for expected in [initialized, first, second] {
        let restored = manager
            .registry
            .workspace_id(&expected.id)
            .unwrap()
            .unwrap();
        assert_eq!(restored.state, WorkspaceState::Active);
        assert_eq!(restored.path, expected.path);
        assert!(restored.path.exists());
        marker::verify(&restored.path, &restored.id).unwrap();
    }
}

#[test]
fn root_unregistration_rolls_back_when_marker_removal_fails() {
    let temp = TempDir::new().unwrap();
    let root = root(&temp);
    let mut manager = manager(&temp);
    let initialized = manager
        .init(InitWorkspace {
            at: root.clone(),
            here: true,
            strategy: InitStrategy::CopyOnWrite,
        })
        .unwrap()
        .workspace;

    let error = manager
        .unregister_root_with(&initialized, "test-removal", |_| {
            Err(std::io::Error::other("simulated marker removal failure").into())
        })
        .unwrap_err();

    assert!(matches!(error, Error::Io(_)));
    assert_eq!(
        manager
            .registry
            .workspace_id(&initialized.id)
            .unwrap()
            .unwrap()
            .state,
        WorkspaceState::Active
    );
    marker::verify(&root, &initialized.id).unwrap();
    assert_eq!(manager.current(&root).unwrap().id, initialized.id);
}

#[test]
fn failed_root_unregistration_restores_trashed_descendants() {
    let temp = TempDir::new().unwrap();
    let root = root(&temp);
    let mut manager = manager(&temp);
    let initialized = manager
        .init(InitWorkspace {
            at: root.clone(),
            here: true,
            strategy: InitStrategy::CopyOnWrite,
        })
        .unwrap()
        .workspace;
    let child = manager
        .create(CreateWorkspace {
            from: root.clone(),
            handle: Some("child".into()),
            into: None,
            copy_mode: CopyMode::All,
        })
        .unwrap()
        .workspace;
    let trash = trash_path(&child.path, &child.id).unwrap();

    let error = manager
        .remove_with_marker_removal(&root, None, RemoveMode::Subtree, true, |_| {
            Err(std::io::Error::other("simulated marker removal failure").into())
        })
        .unwrap_err();

    assert!(matches!(error, Error::Io(_)));
    let restored_child = manager.registry.workspace_id(&child.id).unwrap().unwrap();
    assert_eq!(restored_child.state, WorkspaceState::Active);
    assert_eq!(restored_child.path, child.path);
    assert_eq!(restored_child.original_path, None);
    assert!(restored_child.path.exists());
    assert!(!trash.exists());
    assert_eq!(
        manager
            .registry
            .workspace_id(&initialized.id)
            .unwrap()
            .unwrap()
            .state,
        WorkspaceState::Active
    );
    marker::verify(&root, &initialized.id).unwrap();
}

#[test]
fn failed_root_restore_removes_the_recreated_marker() {
    let temp = TempDir::new().unwrap();
    let root = root(&temp);
    let mut manager = manager(&temp);
    let initialized = manager
        .init(InitWorkspace {
            at: root.clone(),
            here: true,
            strategy: InitStrategy::CopyOnWrite,
        })
        .unwrap()
        .workspace;
    let child = manager
        .create(CreateWorkspace {
            from: root.clone(),
            handle: Some("child".into()),
            into: None,
            copy_mode: CopyMode::All,
        })
        .unwrap()
        .workspace;
    manager
        .remove(&root, None, RemoveMode::Subtree, true)
        .unwrap();

    let trashed = manager.registry.workspace_id(&child.id).unwrap().unwrap();
    let relocated_trash = temp.path().join("relocated-trash");
    fs::rename(&trashed.path, &relocated_trash).unwrap();
    let relocated_trash = fs::canonicalize(relocated_trash).unwrap();
    manager
        .registry
        .update_path(&child.id, &relocated_trash)
        .unwrap();
    let original_parent = trashed
        .original_path
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    fs::remove_dir_all(&original_parent).unwrap();
    fs::write(&original_parent, "blocks restore").unwrap();

    assert!(manager.restore(&root, root.to_str().unwrap()).is_err());
    assert_eq!(marker::read(&root).unwrap(), None);
    assert_eq!(
        manager
            .registry
            .workspace_id(&initialized.id)
            .unwrap()
            .unwrap()
            .state,
        WorkspaceState::Unregistered
    );
    assert_eq!(
        manager
            .registry
            .workspace_id(&child.id)
            .unwrap()
            .unwrap()
            .state,
        WorkspaceState::Trashed
    );
}

#[test]
fn root_removal_requires_force_and_preserves_root_directory() {
    let temp = TempDir::new().unwrap();
    let root = root(&temp);
    let mut manager = manager(&temp);
    manager
        .init(InitWorkspace {
            at: root.clone(),
            here: true,
            strategy: InitStrategy::CopyOnWrite,
        })
        .unwrap();

    assert!(matches!(
        manager.remove(&root, None, RemoveMode::Subtree, false),
        Err(Error::RootForceRequired(_))
    ));
    let removed = manager
        .remove(&root, None, RemoveMode::Subtree, true)
        .unwrap();
    assert_eq!(removed.selected.state, WorkspaceState::Unregistered);
    assert!(removed.removed.is_empty());
    assert!(root.exists());
    assert!(!root.join(MARKER_FILE).exists());
    assert!(
        manager
            .init(InitWorkspace {
                at: root,
                here: true,
                strategy: InitStrategy::CopyOnWrite,
            })
            .is_err()
    );
}

#[cfg(unix)]
#[test]
fn adoption_revalidates_native_cow_storage_compatibility() {
    use std::os::unix::fs::{MetadataExt, symlink};

    let temp = TempDir::new().unwrap();
    let source_parent = temp.path().join("source-parent");
    let root = source_parent.join("project");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("file.txt"), "root").unwrap();
    let mut manager = manager(&temp);
    let initialized = manager
        .init(InitWorkspace {
            at: root.clone(),
            here: true,
            strategy: InitStrategy::CopyOnWrite,
        })
        .unwrap()
        .workspace;
    let source_device = fs::metadata(&root).unwrap().dev();
    let other_filesystem = ["/dev", "/proc", "/sys"]
        .into_iter()
        .map(Path::new)
        .find(|path| {
            fs::metadata(path)
                .map(|metadata| metadata.dev() != source_device)
                .unwrap_or(false)
        });
    let Some(other_filesystem) = other_filesystem else {
        return;
    };

    let moved = temp.path().join("moved-project");
    fs::rename(&root, &moved).unwrap();
    fs::remove_dir(&source_parent).unwrap();
    symlink(other_filesystem, &source_parent).unwrap();

    assert!(matches!(
        manager.adopt(&moved),
        Err(Error::CowUnavailable(_))
    ));
    assert_eq!(
        manager
            .registry
            .workspace_id(&initialized.id)
            .unwrap()
            .unwrap()
            .path,
        initialized.path
    );
}

#[test]
fn adoption_rejects_a_workspace_moved_inside_another_workspace() {
    let temp = TempDir::new().unwrap();
    let first = root(&temp);
    let second = temp.path().join("host");
    fs::create_dir(&second).unwrap();
    let mut manager = manager(&temp);
    manager
        .init(InitWorkspace {
            at: first.clone(),
            here: true,
            strategy: InitStrategy::CopyOnWrite,
        })
        .unwrap();
    manager
        .init(InitWorkspace {
            at: second.clone(),
            here: true,
            strategy: InitStrategy::CopyOnWrite,
        })
        .unwrap();
    let moved = second.join("nested-workspace");
    fs::rename(&first, &moved).unwrap();

    assert!(matches!(
        manager.adopt(&moved),
        Err(Error::InsideManagedWorkspace(path)) if path == fs::canonicalize(&moved).unwrap()
    ));
}

#[test]
fn adoption_rejects_a_path_containing_another_managed_workspace() {
    let temp = TempDir::new().unwrap();
    let moving = root(&temp);
    let container = temp.path().join("container");
    let nested = container.join("nested");
    fs::create_dir_all(&nested).unwrap();
    let mut manager = manager(&temp);
    let moving_workspace = manager
        .init(InitWorkspace {
            at: moving.clone(),
            here: true,
            strategy: InitStrategy::CopyOnWrite,
        })
        .unwrap()
        .workspace;
    manager
        .init(InitWorkspace {
            at: nested.clone(),
            here: true,
            strategy: InitStrategy::CopyOnWrite,
        })
        .unwrap();
    fs::copy(moving.join(MARKER_FILE), container.join(MARKER_FILE)).unwrap();
    fs::remove_dir_all(&moving).unwrap();
    let canonical_nested = fs::canonicalize(&nested).unwrap();

    assert!(matches!(
        manager.adopt(&container),
        Err(Error::ContainsManagedWorkspace(path)) if path == canonical_nested
    ));
    assert_eq!(
        manager
            .registry
            .workspace_id(&moving_workspace.id)
            .unwrap()
            .unwrap()
            .path,
        moving_workspace.path
    );
    assert!(nested.exists());
}

#[cfg(unix)]
#[test]
fn gc_removes_workspaces_with_read_only_directories() {
    use std::os::unix::fs::PermissionsExt;

    let temp = TempDir::new().unwrap();
    let root = root(&temp);
    let mut manager = manager(&temp);
    manager
        .init(InitWorkspace {
            at: root.clone(),
            here: true,
            strategy: InitStrategy::CopyOnWrite,
        })
        .unwrap();
    let child = manager
        .create(CreateWorkspace {
            from: root.clone(),
            handle: Some("child".into()),
            into: None,
            copy_mode: CopyMode::All,
        })
        .unwrap()
        .workspace;
    let read_only = child.path.join("read-only");
    fs::create_dir(&read_only).unwrap();
    fs::write(read_only.join("file.txt"), "contents").unwrap();
    fs::set_permissions(&read_only, fs::Permissions::from_mode(0o555)).unwrap();
    manager
        .remove(&root, Some("child"), RemoveMode::Subtree, false)
        .unwrap();
    let trash = manager
        .registry
        .workspace_id(&child.id)
        .unwrap()
        .unwrap()
        .path;

    let collected = manager.gc().unwrap();

    assert_eq!(collected.deleted, vec![trash.clone()]);
    assert!(!trash.exists());
    assert!(manager.registry.workspace_id(&child.id).unwrap().is_none());
}

#[test]
fn gc_restores_a_marker_removed_by_a_failed_strategy_before_retrying() {
    let temp = TempDir::new().unwrap();
    let root = root(&temp);
    let mut manager = Manager::with_strategy(
        temp.path().join("workspaces.sqlite"),
        Box::new(FailOnceGcStrategy(std::cell::Cell::new(false))),
    )
    .unwrap();
    manager
        .init(InitWorkspace {
            at: root.clone(),
            here: true,
            strategy: InitStrategy::CopyOnWrite,
        })
        .unwrap();
    let child = manager
        .create(CreateWorkspace {
            from: root.clone(),
            handle: Some("child".into()),
            into: None,
            copy_mode: CopyMode::All,
        })
        .unwrap()
        .workspace;
    manager
        .remove(&root, Some("child"), RemoveMode::Subtree, false)
        .unwrap();
    let trash = manager
        .registry
        .workspace_id(&child.id)
        .unwrap()
        .unwrap()
        .path;

    assert!(matches!(manager.gc(), Err(Error::Io(_))));
    marker::verify(&trash, &child.id).unwrap();
    assert!(manager.registry.workspace_id(&child.id).unwrap().is_some());

    let collected = manager.gc().unwrap();
    assert_eq!(collected.deleted, vec![trash.clone()]);
    assert!(!trash.exists());
    assert!(manager.registry.workspace_id(&child.id).unwrap().is_none());
}

#[test]
fn gc_preserves_a_workspace_restored_before_the_registry_commit() {
    let temp = TempDir::new().unwrap();
    let root = root(&temp);
    let mut manager = manager(&temp);
    manager
        .init(InitWorkspace {
            at: root.clone(),
            here: true,
            strategy: InitStrategy::CopyOnWrite,
        })
        .unwrap();
    let child = manager
        .create(CreateWorkspace {
            from: root.clone(),
            handle: Some("child".into()),
            into: None,
            copy_mode: CopyMode::All,
        })
        .unwrap()
        .workspace;
    manager
        .remove(&root, Some("child"), RemoveMode::Subtree, false)
        .unwrap();
    let trashed = manager.registry.workspace_id(&child.id).unwrap().unwrap();
    fs::rename(&trashed.path, &child.path).unwrap();

    assert!(matches!(
        manager.gc(),
        Err(Error::InterruptedRestore(path)) if path == child.path
    ));
    assert_eq!(
        manager
            .registry
            .workspace_id(&child.id)
            .unwrap()
            .unwrap()
            .state,
        WorkspaceState::Trashed
    );
    marker::verify(&child.path, &child.id).unwrap();
}

#[test]
fn gc_preserves_an_unregistered_root_with_a_restored_marker() {
    let temp = TempDir::new().unwrap();
    let root = root(&temp);
    let mut manager = manager(&temp);
    let initialized = manager
        .init(InitWorkspace {
            at: root.clone(),
            here: true,
            strategy: InitStrategy::CopyOnWrite,
        })
        .unwrap()
        .workspace;
    let child = manager
        .create(CreateWorkspace {
            from: root.clone(),
            handle: Some("child".into()),
            into: None,
            copy_mode: CopyMode::All,
        })
        .unwrap()
        .workspace;
    manager
        .remove(&root, None, RemoveMode::Subtree, true)
        .unwrap();
    let trashed_child = manager.registry.workspace_id(&child.id).unwrap().unwrap();
    marker::write(&root, &initialized.id).unwrap();

    assert!(matches!(
        manager.gc(),
        Err(Error::InterruptedRestore(path)) if path == initialized.path
    ));
    assert!(
        manager
            .registry
            .workspace_id(&initialized.id)
            .unwrap()
            .is_some()
    );
    assert!(manager.registry.workspace_id(&child.id).unwrap().is_some());
    assert!(trashed_child.path.exists());
    marker::verify(&root, &initialized.id).unwrap();
}

#[test]
fn gc_refuses_to_delete_replaced_trash() {
    let temp = TempDir::new().unwrap();
    let root = root(&temp);
    let mut manager = manager(&temp);
    manager
        .init(InitWorkspace {
            at: root.clone(),
            here: true,
            strategy: InitStrategy::CopyOnWrite,
        })
        .unwrap();
    let child = manager
        .create(CreateWorkspace {
            from: root.clone(),
            handle: Some("child".into()),
            into: None,
            copy_mode: CopyMode::All,
        })
        .unwrap();
    manager
        .remove(&root, Some("child"), RemoveMode::Subtree, false)
        .unwrap();
    let trashed = manager
        .registry
        .workspace_id(&child.workspace.id)
        .unwrap()
        .unwrap();
    fs::remove_dir_all(&trashed.path).unwrap();
    fs::create_dir(&trashed.path).unwrap();
    fs::write(trashed.path.join("unrelated.txt"), "keep").unwrap();
    marker::write(&trashed.path, &Ulid::new().to_string()).unwrap();

    assert!(matches!(manager.gc(), Err(Error::MarkerMismatch(_))));
    assert_eq!(
        fs::read_to_string(trashed.path.join("unrelated.txt")).unwrap(),
        "keep"
    );
    assert!(
        manager
            .registry
            .workspace_id(&child.workspace.id)
            .unwrap()
            .is_some()
    );
}

#[cfg(unix)]
#[test]
fn gc_rejects_a_symlinked_trashed_workspace_without_deleting_its_target() {
    use std::os::unix::fs::symlink;

    let temp = TempDir::new().unwrap();
    let root = root(&temp);
    let external = temp.path().join("external");
    let mut manager = manager(&temp);
    manager
        .init(InitWorkspace {
            at: root.clone(),
            here: true,
            strategy: InitStrategy::CopyOnWrite,
        })
        .unwrap();
    let child = manager
        .create(CreateWorkspace {
            from: root.clone(),
            handle: Some("child".into()),
            into: None,
            copy_mode: CopyMode::All,
        })
        .unwrap()
        .workspace;
    manager
        .remove(&root, Some("child"), RemoveMode::Subtree, false)
        .unwrap();
    let trashed = manager.registry.workspace_id(&child.id).unwrap().unwrap();
    fs::remove_dir_all(&trashed.path).unwrap();
    fs::create_dir(&external).unwrap();
    let external = fs::canonicalize(external).unwrap();
    fs::write(external.join("outside.txt"), "keep").unwrap();
    marker::write(&external, &child.id).unwrap();
    symlink(&external, &trashed.path).unwrap();

    assert!(matches!(
        manager.gc(),
        Err(Error::InvalidMarker(path)) if path == trashed.path
    ));
    assert_eq!(
        fs::read_to_string(external.join("outside.txt")).unwrap(),
        "keep"
    );
    marker::verify(&external, &child.id).unwrap();
    assert!(manager.registry.workspace_id(&child.id).unwrap().is_some());
}

#[test]
fn gc_propagates_trash_existence_check_errors() {
    let temp = TempDir::new().unwrap();
    let root = root(&temp);
    let mut manager = manager(&temp);
    manager
        .init(InitWorkspace {
            at: root.clone(),
            here: true,
            strategy: InitStrategy::CopyOnWrite,
        })
        .unwrap();
    let child = manager
        .create(CreateWorkspace {
            from: root.clone(),
            handle: Some("child".into()),
            into: None,
            copy_mode: CopyMode::All,
        })
        .unwrap()
        .workspace;
    manager
        .remove(&root, Some("child"), RemoveMode::Subtree, false)
        .unwrap();
    let blocker = temp.path().join("blocker");
    fs::write(&blocker, "not a directory").unwrap();
    manager
        .registry
        .update_path(&child.id, &blocker.join("trash"))
        .unwrap();

    assert!(matches!(manager.gc(), Err(Error::Io(_))));
    assert!(manager.registry.workspace_id(&child.id).unwrap().is_some());
}

#[test]
fn doctor_removes_an_interrupted_clone_with_the_source_marker() {
    let temp = TempDir::new().unwrap();
    let root = root(&temp);
    let mut manager = manager(&temp);
    let initialized = manager
        .init(InitWorkspace {
            at: root,
            here: true,
            strategy: InitStrategy::CopyOnWrite,
        })
        .unwrap()
        .workspace;

    let creating_id = Ulid::new().to_string();
    let staging = temp.path().join("inherited-marker-staging");
    fs::create_dir(&staging).unwrap();
    let staging = fs::canonicalize(staging).unwrap();
    marker::write(&staging, &initialized.id).unwrap();
    fs::write(staging.join("partial.txt"), "partial").unwrap();
    manager
        .registry
        .insert_creating(
            &creating_id,
            &initialized.root_id,
            &initialized.id,
            "inherited-marker",
            &temp.path().join("inherited-marker-destination"),
            &staging,
            &initialized.strategy,
            CopyMode::All,
        )
        .unwrap();

    let report = manager.doctor(true).unwrap();

    assert!(
        report
            .issues
            .iter()
            .any(|issue| issue.workspace_id == creating_id && issue.fixed)
    );
    assert!(!staging.exists());
    assert!(
        manager
            .registry
            .workspace_id(&creating_id)
            .unwrap()
            .is_none()
    );
    marker::verify(&initialized.path, &initialized.id).unwrap();
}

#[test]
fn doctor_only_deletes_owned_interrupted_create_staging() {
    let temp = TempDir::new().unwrap();
    let root = root(&temp);
    let mut manager = manager(&temp);
    let initialized = manager
        .init(InitWorkspace {
            at: root,
            here: true,
            strategy: InitStrategy::CopyOnWrite,
        })
        .unwrap()
        .workspace;

    let replaced_id = Ulid::new().to_string();
    let replaced_staging = temp.path().join("replaced-staging");
    fs::create_dir(&replaced_staging).unwrap();
    let replaced_staging = fs::canonicalize(replaced_staging).unwrap();
    fs::write(replaced_staging.join("unrelated.txt"), "keep").unwrap();
    manager
        .registry
        .insert_creating(
            &replaced_id,
            &initialized.root_id,
            &initialized.id,
            "replaced-staging",
            &temp.path().join("replaced-destination"),
            &replaced_staging,
            &initialized.strategy,
            CopyMode::All,
        )
        .unwrap();

    let owned_id = Ulid::new().to_string();
    let owned_staging = temp.path().join("owned-staging");
    fs::create_dir(&owned_staging).unwrap();
    let owned_staging = fs::canonicalize(owned_staging).unwrap();
    marker::write(&owned_staging, &owned_id).unwrap();
    fs::write(owned_staging.join("partial.txt"), "partial").unwrap();
    manager
        .registry
        .insert_creating(
            &owned_id,
            &initialized.root_id,
            &initialized.id,
            "owned-staging",
            &temp.path().join("owned-destination"),
            &owned_staging,
            &initialized.strategy,
            CopyMode::All,
        )
        .unwrap();

    let report = manager.doctor(true).unwrap();

    let replaced_issue = report
        .issues
        .iter()
        .find(|issue| issue.workspace_id == replaced_id)
        .unwrap();
    assert!(!replaced_issue.fixed);
    assert_eq!(replaced_issue.path, replaced_staging);
    assert_eq!(
        replaced_issue.message,
        "interrupted-create staging marker is missing"
    );
    assert_eq!(
        fs::read_to_string(replaced_staging.join("unrelated.txt")).unwrap(),
        "keep"
    );
    assert!(
        manager
            .registry
            .workspace_id(&replaced_id)
            .unwrap()
            .is_some()
    );

    assert!(
        report
            .issues
            .iter()
            .any(|issue| issue.workspace_id == owned_id && issue.fixed)
    );
    assert!(!owned_staging.exists());
    assert!(manager.registry.workspace_id(&owned_id).unwrap().is_none());
}

#[test]
fn doctor_does_not_delete_replaced_interrupted_create_destinations() {
    let temp = TempDir::new().unwrap();
    let root = root(&temp);
    let mut manager = manager(&temp);
    let initialized = manager
        .init(InitWorkspace {
            at: root,
            here: true,
            strategy: InitStrategy::CopyOnWrite,
        })
        .unwrap()
        .workspace;

    let missing_id = Ulid::new().to_string();
    let missing_destination = temp.path().join("missing-destination");
    fs::create_dir(&missing_destination).unwrap();
    let missing_destination = fs::canonicalize(missing_destination).unwrap();
    fs::write(missing_destination.join("unrelated.txt"), "keep missing").unwrap();
    manager
        .registry
        .insert_creating(
            &missing_id,
            &initialized.root_id,
            &initialized.id,
            "missing-create",
            &missing_destination,
            &temp.path().join(".missing-staging"),
            &initialized.strategy,
            CopyMode::All,
        )
        .unwrap();

    let mismatch_id = Ulid::new().to_string();
    let mismatch_destination = temp.path().join("mismatch-destination");
    fs::create_dir(&mismatch_destination).unwrap();
    let mismatch_destination = fs::canonicalize(mismatch_destination).unwrap();
    fs::write(mismatch_destination.join("unrelated.txt"), "keep mismatch").unwrap();
    marker::write(&mismatch_destination, &Ulid::new().to_string()).unwrap();
    manager
        .registry
        .insert_creating(
            &mismatch_id,
            &initialized.root_id,
            &initialized.id,
            "mismatch-create",
            &mismatch_destination,
            &temp.path().join(".mismatch-staging"),
            &initialized.strategy,
            CopyMode::All,
        )
        .unwrap();

    let valid_id = Ulid::new().to_string();
    let valid_destination = temp.path().join("valid-destination");
    fs::create_dir(&valid_destination).unwrap();
    let valid_destination = fs::canonicalize(valid_destination).unwrap();
    marker::write(&valid_destination, &valid_id).unwrap();
    manager
        .registry
        .insert_creating(
            &valid_id,
            &initialized.root_id,
            &initialized.id,
            "valid-create",
            &valid_destination,
            &temp.path().join(".valid-staging"),
            &initialized.strategy,
            CopyMode::All,
        )
        .unwrap();

    let report = manager.doctor(true).unwrap();

    let missing_issue = report
        .issues
        .iter()
        .find(|issue| issue.workspace_id == missing_id)
        .unwrap();
    assert!(!missing_issue.fixed);
    assert_eq!(
        missing_issue.message,
        "interrupted-create destination marker is missing"
    );
    let mismatch_issue = report
        .issues
        .iter()
        .find(|issue| issue.workspace_id == mismatch_id)
        .unwrap();
    assert!(!mismatch_issue.fixed);
    assert_eq!(
        mismatch_issue.message,
        "interrupted-create destination marker belongs to another workspace"
    );
    assert_eq!(
        fs::read_to_string(missing_destination.join("unrelated.txt")).unwrap(),
        "keep missing"
    );
    assert_eq!(
        fs::read_to_string(mismatch_destination.join("unrelated.txt")).unwrap(),
        "keep mismatch"
    );
    assert!(
        manager
            .registry
            .workspace_id(&missing_id)
            .unwrap()
            .is_some()
    );
    assert!(
        manager
            .registry
            .workspace_id(&mismatch_id)
            .unwrap()
            .is_some()
    );
    assert!(
        report
            .issues
            .iter()
            .any(|issue| issue.workspace_id == valid_id && issue.fixed)
    );
    assert!(!valid_destination.exists());
    assert!(manager.registry.workspace_id(&valid_id).unwrap().is_none());
}

#[test]
fn doctor_reports_invalid_markers_and_continues_scanning() {
    let temp = TempDir::new().unwrap();
    let root = root(&temp);
    let mut manager = manager(&temp);
    manager
        .init(InitWorkspace {
            at: root.clone(),
            here: true,
            strategy: InitStrategy::CopyOnWrite,
        })
        .unwrap();
    let child = manager
        .create(CreateWorkspace {
            from: root.clone(),
            handle: Some("child".into()),
            into: None,
            copy_mode: CopyMode::All,
        })
        .unwrap();
    fs::write(root.join(MARKER_FILE), "not-a-ulid\n").unwrap();
    fs::remove_file(child.workspace.path.join(MARKER_FILE)).unwrap();

    let report = manager.doctor(false).unwrap();

    assert_eq!(report.issues.len(), 2);
    assert!(
        report
            .issues
            .iter()
            .any(|issue| issue.message == "workspace marker is invalid" && !issue.fixed)
    );
    assert!(
        report
            .issues
            .iter()
            .any(|issue| issue.message == "workspace marker is missing" && !issue.fixed)
    );
}

#[test]
fn target_resolution_rejects_a_replaced_active_workspace() {
    let temp = TempDir::new().unwrap();
    let root = root(&temp);
    let mut manager = manager(&temp);
    manager
        .init(InitWorkspace {
            at: root.clone(),
            here: true,
            strategy: InitStrategy::CopyOnWrite,
        })
        .unwrap();
    let child = manager
        .create(CreateWorkspace {
            from: root.clone(),
            handle: Some("child".into()),
            into: None,
            copy_mode: CopyMode::All,
        })
        .unwrap()
        .workspace;
    fs::remove_dir_all(&child.path).unwrap();
    fs::create_dir(&child.path).unwrap();
    fs::write(child.path.join("unrelated.txt"), "keep").unwrap();

    assert!(matches!(
        manager.resolve_target(&root, Some("child"), false),
        Err(Error::MissingMarker(path)) if path == child.path
    ));
    assert_eq!(
        fs::read_to_string(child.path.join("unrelated.txt")).unwrap(),
        "keep"
    );
}

#[test]
fn ancestors_reject_a_replaced_active_parent() {
    let temp = TempDir::new().unwrap();
    let root = root(&temp);
    let mut manager = manager(&temp);
    manager
        .init(InitWorkspace {
            at: root.clone(),
            here: true,
            strategy: InitStrategy::CopyOnWrite,
        })
        .unwrap();
    let child = manager
        .create(CreateWorkspace {
            from: root.clone(),
            handle: Some("child".into()),
            into: None,
            copy_mode: CopyMode::All,
        })
        .unwrap()
        .workspace;
    let canonical_root = fs::canonicalize(&root).unwrap();
    fs::remove_dir_all(&root).unwrap();
    fs::create_dir(&root).unwrap();
    fs::write(root.join("unrelated.txt"), "keep").unwrap();

    assert!(matches!(
        manager.ancestors(&child.path, None),
        Err(Error::MissingMarker(path)) if path == canonical_root
    ));
    assert_eq!(
        fs::read_to_string(root.join("unrelated.txt")).unwrap(),
        "keep"
    );
}

#[test]
fn exact_handle_takes_precedence_over_an_existing_relative_directory() {
    let _current_directory_guard = CURRENT_DIRECTORY_TEST_LOCK.lock().unwrap();
    let handle = fs::read_dir(std::env::current_dir().unwrap())
        .unwrap()
        .filter_map(std::result::Result::ok)
        .find_map(|entry| {
            let handle = entry.file_name().into_string().ok()?;
            (entry.file_type().ok()?.is_dir()
                && !matches!(handle.as_str(), "project" | "context")
                && validate_handle(handle.clone()).is_ok())
            .then_some(handle)
        })
        .expect("the test working directory should contain a usable directory name");
    assert!(Path::new(&handle).is_dir());

    let temp = TempDir::new().unwrap();
    let root = root(&temp);
    let mut manager = manager(&temp);
    manager
        .init(InitWorkspace {
            at: root.clone(),
            here: true,
            strategy: InitStrategy::CopyOnWrite,
        })
        .unwrap();
    let expected = manager
        .create(CreateWorkspace {
            from: root.clone(),
            handle: Some(handle.clone()),
            into: None,
            copy_mode: CopyMode::All,
        })
        .unwrap()
        .workspace;
    let context = manager
        .create(CreateWorkspace {
            from: root,
            handle: Some("context".into()),
            into: None,
            copy_mode: CopyMode::All,
        })
        .unwrap();

    let resolved = manager
        .resolve_target(&context.workspace.path, Some(&handle), false)
        .unwrap();

    assert_eq!(resolved.id, expected.id);
}

#[test]
fn id_prefix_takes_precedence_over_an_existing_relative_directory() {
    let _current_directory_guard = CURRENT_DIRECTORY_TEST_LOCK.lock().unwrap();
    let temp = TempDir::new().unwrap();
    let root = root(&temp);
    let mut manager = manager(&temp);
    manager
        .init(InitWorkspace {
            at: root.clone(),
            here: true,
            strategy: InitStrategy::CopyOnWrite,
        })
        .unwrap();
    let expected = manager
        .create(CreateWorkspace {
            from: root.clone(),
            handle: Some("expected".into()),
            into: None,
            copy_mode: CopyMode::All,
        })
        .unwrap()
        .workspace;
    let prefix = (8..expected.id.len())
        .map(|length| &expected.id[..length])
        .find(|prefix| {
            !Path::new(prefix).exists()
                && manager
                    .registry
                    .find_target(&expected.root_id, prefix, false)
                    .unwrap()
                    .len()
                    == 1
        })
        .expect("the workspace should have an unused unambiguous ID prefix");
    fs::create_dir(prefix).unwrap();

    let resolved = manager.resolve_target(&root, Some(prefix), false);
    fs::remove_dir(prefix).unwrap();

    assert_eq!(resolved.unwrap().id, expected.id);
}

#[test]
fn wildcard_handle_characters_are_literal_in_target_lookups() {
    let temp = TempDir::new().unwrap();
    let root = root(&temp);
    let mut manager = manager(&temp);
    manager
        .init(InitWorkspace {
            at: root.clone(),
            here: true,
            strategy: InitStrategy::CopyOnWrite,
        })
        .unwrap();

    assert!(matches!(
        manager.resolve_target(&root, Some("%"), false),
        Err(Error::UnknownWorkspace(target)) if target == "%"
    ));
    for handle in ["%", "_"] {
        let created = manager
            .create(CreateWorkspace {
                from: root.clone(),
                handle: Some(handle.into()),
                into: None,
                copy_mode: CopyMode::All,
            })
            .unwrap();
        assert_eq!(
            manager
                .resolve_target(&root, Some(handle), false)
                .unwrap()
                .id,
            created.workspace.id
        );
    }
}

#[cfg(unix)]
#[test]
fn cow_storage_validation_rejects_another_filesystem() {
    use std::os::unix::fs::MetadataExt;

    let temp = TempDir::new().unwrap();
    let root = root(&temp);
    let source_device = fs::metadata(&root).unwrap().dev();
    let other_filesystem = ["/proc", "/dev", "/sys"]
        .into_iter()
        .map(Path::new)
        .find(|path| {
            fs::metadata(path)
                .map(|metadata| metadata.dev() != source_device)
                .unwrap_or(false)
        });
    let Some(other_filesystem) = other_filesystem else {
        return;
    };
    let storage = other_filesystem.join("hz-storage-does-not-exist");

    assert!(matches!(
        ensure_cow_storage_compatible(&root, &storage),
        Err(Error::CowUnavailable(_))
    ));
}

#[test]
fn workspace_handles_reject_line_breaking_and_control_characters() {
    for handle in [
        "one\ntwo",
        "one\rtwo",
        "one\ttwo",
        "one\u{1b}two",
        "one\u{85}two",
        "one\u{2028}two",
        "one\u{2029}two",
    ] {
        assert!(matches!(
            validate_handle(handle.into()),
            Err(Error::Path(_))
        ));
    }
}

#[test]
fn init_rejects_a_reserved_root_handle() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("current");
    fs::create_dir(&root).unwrap();
    let mut manager = manager(&temp);

    assert!(matches!(
        manager.init(InitWorkspace {
            at: root.clone(),
            here: true,
            strategy: InitStrategy::CopyOnWrite,
        }),
        Err(Error::Path(_))
    ));
    assert_eq!(marker::read(&root).unwrap(), None);
    assert!(manager.registry.all_records().unwrap().is_empty());
}

#[cfg(unix)]
#[test]
fn init_rejects_a_line_breaking_root_handle() {
    let temp = TempDir::new().unwrap();
    let root = temp.path().join("one\ntwo");
    fs::create_dir(&root).unwrap();
    let mut manager = manager(&temp);

    assert!(matches!(
        manager.init(InitWorkspace {
            at: root.clone(),
            here: true,
            strategy: InitStrategy::CopyOnWrite,
        }),
        Err(Error::Path(_))
    ));
    assert_eq!(marker::read(&root).unwrap(), None);
    assert!(manager.registry.all_records().unwrap().is_empty());
}

#[test]
fn unknown_workspace_errors_keep_their_stable_classification() {
    let error = hz_core::HzError::from(Error::UnknownWorkspace("missing".into()));
    assert!(matches!(
        error,
        hz_core::HzError::UnknownWorkspace { target } if target == "missing"
    ));
}
