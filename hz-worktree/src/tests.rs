use super::*;
use std::{
    collections::HashSet,
    env, fs,
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use hz_core::{HzError, paths::WorktreeTarget};

#[test]
fn generated_handle_is_easy_to_type() {
    let handle = generate_handle_from_seed(0, 0);

    assert_eq!(handle.len(), 4);
    assert!(
        handle
            .chars()
            .all(|character| { character.is_ascii_lowercase() || character.is_ascii_digit() })
    );
}

#[test]
fn generated_handle_space_is_four_lowercase_alphanumeric_characters() {
    assert_eq!(HANDLE_ALPHABET.len(), 36);
    assert_eq!(HANDLE_LENGTH, 4);
    assert_eq!(handle_space_size(), 1_679_616);

    assert_eq!(generate_handle_from_seed(0, 0), "aaaa");
    assert_eq!(generate_handle_from_seed(0, 35), "aaa9");
    assert_eq!(generate_handle_from_seed(0, 36), "aaba");
}

#[test]
fn generated_handle_mixes_timestamp_shaped_seeds() {
    let base = 1_700_000_000_000_000_000_u128;
    let mut handles = (0..8)
        .map(|index| generate_handle_from_seed(base + index * 1_000_000_000, 0))
        .collect::<Vec<_>>();
    handles.sort();
    handles.dedup();

    assert!(
        handles.len() > 1,
        "timestamp-shaped seeds should not collapse to one handle"
    );
}

#[test]
fn worktree_branch_derivation_keeps_only_unnamed_worktrees_detached() {
    assert_eq!(derive_worktree_branch(None, None), None);
    assert_eq!(
        derive_worktree_branch(Some("feature/ui"), None).as_deref(),
        Some("feature/ui")
    );
    assert_eq!(
        derive_worktree_branch(None, Some("feature/explicit")).as_deref(),
        Some("feature/explicit")
    );
}

#[test]
fn detached_prune_candidates_select_oldest_clean_managed_detached_worktree() {
    let repo = PathBuf::from("/repo/hz");
    let old = detached_test_entry(&repo, "old", 1);
    let new = detached_test_entry(&repo, "new", 2);
    let branch_backed = WorktreeEntry {
        branch: Some("feature/ui".to_owned()),
        ..detached_test_entry(&repo, "branch", 0)
    };
    let unmanaged = WorktreeEntry {
        source: WorktreeSource::Git,
        ..detached_test_entry(&repo, "unmanaged", 0)
    };
    let stale = detached_test_entry(&repo, "stale", 0);
    let registry = Registry {
        entries: vec![new.clone(), branch_backed, unmanaged, stale, old.clone()],
        handoffs: Vec::new(),
        patch_handoffs: Vec::new(),
    };
    let git_worktrees = vec![
        git_detached(&new),
        git_detached(&old),
        hz_git::GitWorktree {
            path: PathBuf::from("/worktrees/branch"),
            branch: Some("feature/ui".to_owned()),
        },
    ];

    let candidates = select_detached_worktree_prune_candidates(
        &registry,
        &repo,
        2,
        None,
        &git_worktrees,
        |_| true,
    )
    .unwrap();

    assert_eq!(
        candidates
            .iter()
            .map(|entry| entry.handle.as_str())
            .collect::<Vec<_>>(),
        vec!["old"]
    );
}

#[test]
fn detached_prune_candidates_error_when_current_or_dirty_entries_block_limit() {
    let repo = PathBuf::from("/repo/hz");
    let current = detached_test_entry(&repo, "current", 1);
    let dirty = detached_test_entry(&repo, "dirty", 2);
    let registry = Registry {
        entries: vec![current.clone(), dirty.clone()],
        handoffs: Vec::new(),
        patch_handoffs: Vec::new(),
    };
    let git_worktrees = vec![git_detached(&current), git_detached(&dirty)];

    let error = select_detached_worktree_prune_candidates(
        &registry,
        &repo,
        2,
        Some(&current.path),
        &git_worktrees,
        |path| !same_path(path, &dirty.path),
    )
    .unwrap_err();

    assert_eq!(
        error.to_string(),
        "detached worktree limit 2 would be exceeded; not enough clean detached worktrees can be auto-removed"
    );
}

#[test]
fn detached_prune_candidates_allow_zero_to_disable_auto_pruning() {
    let repo = PathBuf::from("/repo/hz");
    let entry = detached_test_entry(&repo, "old", 1);
    let registry = Registry {
        entries: vec![entry.clone()],
        handoffs: Vec::new(),
        patch_handoffs: Vec::new(),
    };

    let candidates = select_detached_worktree_prune_candidates(
        &registry,
        &repo,
        0,
        None,
        &[git_detached(&entry)],
        |_| false,
    )
    .unwrap();

    assert!(candidates.is_empty());
}

#[test]
fn detached_prune_warning_preserves_created_worktree_context() {
    let warning = detached_prune_warning(HzError::Usage("permission denied".to_owned()));

    assert_eq!(
        warning,
        "created worktree, but failed to prune detached worktrees: permission denied"
    );
}

#[test]
fn created_handoff_destination_preserves_prune_warnings() {
    let (entry, warnings) = created_worktree_entry(
        CreatedWorktree {
            id: "entry-id".to_owned(),
            name: "generated-handle".to_owned(),
            handle: "generated-handle".to_owned(),
            repo: PathBuf::from("/repo/hz"),
            path: PathBuf::from("/worktrees/entry"),
            branch: None,
            base: None,
            source: WorktreeSource::Managed,
            warnings: vec![
                "created worktree, but failed to prune detached worktrees: permission denied"
                    .to_owned(),
            ],
        },
        42,
    );

    assert_eq!(entry.handle, "generated-handle");
    assert_eq!(entry.created_at_unix, 42);
    assert_eq!(
        warnings,
        vec!["created worktree, but failed to prune detached worktrees: permission denied"]
    );
}

#[test]
fn create_rolls_back_git_state_when_registry_save_fails() {
    let test_dir = test_dir("hz-worktree-create-save-failure-test");
    let repo = test_dir.join("repo");
    let destination = test_dir.join("destination");
    init_committed_repo(&repo);
    git(["branch", "-m", "main"], &repo);
    let blocked_parent = test_dir.join("blocked-registry-parent");
    fs::write(&blocked_parent, "not a directory")
        .expect("blocked registry parent should be written");
    let _registry_path_override =
        RegistryPathOverrideGuard::set(blocked_parent.join("registry.json"));
    let mut registry = Registry::default();

    let error = create_with_registry(
        &mut registry,
        CreateWorktree {
            name: Some("feature".to_owned()),
            repo: Some(repo.clone()),
            path: Some(destination.clone()),
            base: None,
            branch: None,
            max_detached_worktrees: None,
        },
    )
    .unwrap_err();

    assert!(matches!(&error, HzError::Io(_)), "{error}");
    assert!(registry.entries.is_empty());
    assert!(!destination.exists());
    assert!(!git_worktree_listed(&repo, &destination));
    assert!(!hz_git::branch_exists(&repo, "feature").unwrap());

    fs::remove_dir_all(test_dir).expect("test directory should be removed");
}

#[test]
fn create_rollback_keeps_branch_when_worktree_removal_fails() {
    let test_dir = test_dir("hz-worktree-create-rollback-remove-failure-test");
    let repo = test_dir.join("repo");
    let destination = test_dir.join("destination");
    init_committed_repo(&repo);
    git(["branch", "-m", "main"], &repo);
    git(["branch", "feature"], &repo);
    fs::create_dir_all(&destination).expect("destination directory should be created");

    let error = rollback_created_worktree(
        &repo,
        &destination,
        Some("feature"),
        HzError::Usage("registry save failed".to_owned()),
    );

    assert!(error.to_string().contains("rollback failed"), "{error}");
    assert!(
        hz_git::branch_exists(&repo, "feature").unwrap(),
        "branch should remain when worktree cleanup fails"
    );

    fs::remove_dir_all(test_dir).expect("test directory should be removed");
}

#[test]
fn remove_does_not_remove_git_worktree_when_registry_save_fails() {
    let test_dir = test_dir("hz-worktree-remove-save-failure-test");
    let repo = test_dir.join("repo");
    let destination = test_dir.join("destination");
    init_committed_repo(&repo);
    git(["branch", "-m", "main"], &repo);
    git(["branch", "feature"], &repo);
    git(
        [
            "worktree",
            "add",
            "-q",
            destination.to_str().unwrap(),
            "feature",
        ],
        &repo,
    );
    let destination = fs::canonicalize(destination).unwrap();
    let entry = WorktreeEntry {
        id: "feature".to_owned(),
        handle: "feature".to_owned(),
        repo: repo.clone(),
        path: destination.clone(),
        branch: Some("feature".to_owned()),
        base: None,
        source: WorktreeSource::Managed,
        created_at_unix: 0,
        modified_at_unix: 0,
        status: WorktreeStatus::Unknown,
    };
    let mut registry = Registry {
        entries: vec![entry.clone()],
        handoffs: Vec::new(),
        patch_handoffs: Vec::new(),
    };
    let blocked_parent = test_dir.join("blocked-registry-parent");
    fs::write(&blocked_parent, "not a directory")
        .expect("blocked registry parent should be written");
    let _registry_path_override =
        RegistryPathOverrideGuard::set(blocked_parent.join("registry.json"));

    let error =
        remove_registered_entry_with_force_from_registry(&mut registry, entry.clone(), false)
            .unwrap_err();

    assert!(matches!(&error, HzError::Io(_)), "{error}");
    assert_eq!(registry.entries, vec![entry]);
    assert!(git_worktree_listed(&repo, &destination));
    assert_eq!(
        hz_git::current_branch(&destination).unwrap().as_deref(),
        Some("feature")
    );

    fs::remove_dir_all(test_dir).expect("test directory should be removed");
}

#[test]
fn remove_restores_registry_when_git_removal_fails() {
    let test_dir = test_dir("hz-worktree-remove-git-failure-test");
    let repo = test_dir.join("repo");
    let destination = test_dir.join("destination");
    init_committed_repo(&repo);
    git(["branch", "-m", "main"], &repo);
    git(["branch", "feature"], &repo);
    git(
        [
            "worktree",
            "add",
            "-q",
            destination.to_str().unwrap(),
            "feature",
        ],
        &repo,
    );
    let destination = fs::canonicalize(destination).unwrap();
    fs::write(destination.join("file.txt"), "dirty\n").expect("worktree should be dirtied");
    let entry = WorktreeEntry {
        id: "feature".to_owned(),
        handle: "feature".to_owned(),
        repo: repo.clone(),
        path: destination.clone(),
        branch: Some("feature".to_owned()),
        base: None,
        source: WorktreeSource::Managed,
        created_at_unix: 0,
        modified_at_unix: 0,
        status: WorktreeStatus::Unknown,
    };
    let mut registry = Registry {
        entries: vec![entry.clone()],
        handoffs: Vec::new(),
        patch_handoffs: Vec::new(),
    };
    let registry_path = test_dir.join("config").join("registry.json");
    let _registry_path_override = RegistryPathOverrideGuard::set(registry_path);
    registry.save().expect("registry should be saved");

    let error =
        remove_registered_entry_with_force_from_registry(&mut registry, entry.clone(), false)
            .unwrap_err();

    assert!(
        error.to_string().contains("failed to remove git worktree"),
        "{error}"
    );
    assert_eq!(registry.entries, vec![entry.clone()]);
    assert_eq!(Registry::load().unwrap().entries, vec![entry]);
    assert!(git_worktree_listed(&repo, &destination));

    fs::remove_dir_all(test_dir).expect("test directory should be removed");
}

#[test]
fn prune_does_not_remove_git_worktree_when_registry_save_fails() {
    let test_dir = test_dir("hz-worktree-prune-save-failure-test");
    let repo = test_dir.join("repo");
    let destination = test_dir.join("destination");
    init_committed_repo(&repo);
    git(["branch", "-m", "main"], &repo);
    git(
        [
            "worktree",
            "add",
            "-q",
            "--detach",
            destination.to_str().unwrap(),
            "HEAD",
        ],
        &repo,
    );
    let destination = fs::canonicalize(destination).unwrap();
    let entry = WorktreeEntry {
        id: "detached".to_owned(),
        handle: "detached".to_owned(),
        repo: repo.clone(),
        path: destination.clone(),
        branch: None,
        base: None,
        source: WorktreeSource::Managed,
        created_at_unix: 0,
        modified_at_unix: 0,
        status: WorktreeStatus::Unknown,
    };
    let mut registry = Registry {
        entries: vec![entry.clone()],
        handoffs: Vec::new(),
        patch_handoffs: Vec::new(),
    };
    let blocked_parent = test_dir.join("blocked-registry-parent");
    fs::write(&blocked_parent, "not a directory")
        .expect("blocked registry parent should be written");
    let _registry_path_override =
        RegistryPathOverrideGuard::set(blocked_parent.join("registry.json"));

    let error = prune_detached_worktrees(&mut registry, vec![entry.clone()]).unwrap_err();

    assert!(matches!(&error, HzError::Io(_)), "{error}");
    assert_eq!(registry.entries, vec![entry]);
    assert!(git_worktree_listed(&repo, &destination));
    assert_eq!(hz_git::current_branch(&destination).unwrap(), None);

    fs::remove_dir_all(test_dir).expect("test directory should be removed");
}

#[test]
fn generated_unique_handle_searches_past_old_probe_window() {
    let repo = PathBuf::from("/repo");
    let seed = 0;
    let mut registry = Registry::default();

    for attempt in 0..128 {
        registry
            .entries
            .push(test_entry(&repo, generate_handle_from_seed(seed, attempt)));
    }

    assert_eq!(
        generate_unique_handle_from_seed(&registry, &repo, seed),
        generate_handle_from_seed(seed, 128)
    );
}

#[test]
fn generated_unique_handle_uses_suffix_after_name_space_is_full() {
    let repo = PathBuf::from("/repo");
    let seed = 0;
    let mut registry = Registry::default();

    let max_attempts = 3;
    for attempt in 0..max_attempts {
        registry
            .entries
            .push(test_entry(&repo, generate_handle_from_seed(seed, attempt)));
    }

    assert_eq!(
        generate_unique_handle_from_seed_with_limit(&registry, &repo, seed, max_attempts),
        format!("{}-2", generate_handle_from_seed(seed, max_attempts))
    );
}

#[test]
fn generated_unique_handle_skips_live_worktree_targets() {
    let seed = 0;
    let targets = HashSet::from([generate_handle_from_seed(seed, 0)]);

    assert_eq!(
        generate_unique_handle_from_seed_with_targets(seed, handle_space_size(), &targets),
        generate_handle_from_seed(seed, 1)
    );
}

#[test]
fn generated_unique_handle_suffix_skips_live_worktree_targets() {
    let seed = 0;
    let max_attempts = 1;
    let fallback = generate_handle_from_seed(seed, max_attempts);
    let targets = HashSet::from([generate_handle_from_seed(seed, 0), format!("{fallback}-2")]);

    assert_eq!(
        generate_unique_handle_from_seed_with_targets(seed, max_attempts, &targets),
        format!("{fallback}-3")
    );
}

#[test]
fn local_is_reserved_for_repository_root() {
    assert!(validate_worktree_name("worktree handle", "feature").is_ok());

    let error = validate_worktree_name("worktree handle", "local").unwrap_err();
    assert_eq!(
        error.to_string(),
        "worktree handle 'local' is reserved for the repository root"
    );
}

#[test]
fn relative_worktree_paths_are_resolved_from_repo_root() {
    let repo = PathBuf::from("/repo");

    assert_eq!(
        resolve_worktree_path(&repo, PathBuf::from("../worktree")),
        PathBuf::from("/repo/../worktree")
    );
    assert_eq!(
        resolve_worktree_path(&repo, PathBuf::from("/tmp/worktree")),
        PathBuf::from("/tmp/worktree")
    );
}

#[test]
fn repo_resolution_uses_registered_repo_for_managed_linked_worktree() {
    let repo = PathBuf::from("/repo/hz");
    let linked = PathBuf::from("/worktrees/managed");
    let registry = Registry {
        entries: vec![WorktreeEntry {
            id: "managed-id".to_owned(),
            handle: "managed".to_owned(),
            repo: repo.clone(),
            path: linked.clone(),
            branch: Some("managed".to_owned()),
            base: None,
            source: WorktreeSource::Managed,
            created_at_unix: 0,
            modified_at_unix: 0,
            status: WorktreeStatus::Unknown,
        }],
        handoffs: Vec::new(),
        patch_handoffs: Vec::new(),
    };

    assert_eq!(
        resolve_registered_repo(&registry, &linked, &repo),
        Some(repo)
    );
}

#[test]
fn repo_resolution_uses_registered_primary_for_unmanaged_linked_worktree() {
    let repo = PathBuf::from("/repo/hz");
    let unmanaged = PathBuf::from("/worktrees/unmanaged");
    let registry = Registry {
        entries: vec![test_entry(&repo, "managed".to_owned())],
        handoffs: Vec::new(),
        patch_handoffs: Vec::new(),
    };

    assert_eq!(
        resolve_registered_repo(&registry, &unmanaged, &repo),
        Some(repo)
    );
}

#[test]
fn repo_resolution_falls_back_when_registry_has_no_repo_match() {
    let repo = PathBuf::from("/repo/hz");
    let other_repo = PathBuf::from("/repo/other");
    let unmanaged = PathBuf::from("/worktrees/unmanaged");
    let registry = Registry {
        entries: vec![test_entry(&other_repo, "managed".to_owned())],
        handoffs: Vec::new(),
        patch_handoffs: Vec::new(),
    };

    assert_eq!(resolve_registered_repo(&registry, &unmanaged, &repo), None);
}

#[test]
fn registry_entries_default_added_state_fields() {
    let registry: Registry = serde_json::from_str(
        r#"{
              "entries": [
                {
                  "id": "managed-id",
                  "handle": "managed",
                  "repo": "/repo/hz",
                  "path": "/worktrees/managed",
                  "branch": "managed",
                  "base": null,
                  "source": "managed",
                  "created_at_unix": 42
                }
              ]
            }"#,
    )
    .unwrap();

    assert_eq!(registry.entries[0].modified_at_unix, 0);
    assert_eq!(registry.entries[0].status, WorktreeStatus::Unknown);
    assert!(registry.handoffs.is_empty());
    assert!(registry.patch_handoffs.is_empty());
}

#[test]
fn registry_remembers_one_handoff_per_branch() {
    let repo = PathBuf::from("/repo/hz");
    let first = PathBuf::from("/worktrees/first");
    let second = PathBuf::from("/worktrees/second");
    let mut registry = Registry::default();

    registry
        .remember_handoff(
            &repo,
            "feature/ui",
            &first,
            "first",
            Some("main".to_owned()),
        )
        .unwrap();
    registry
        .remember_handoff(&repo, "feature/ui", &second, "second", None)
        .unwrap();

    let link = registry.handoff_link(&repo, "feature/ui").unwrap();
    assert_eq!(link.path, second);
    assert_eq!(link.handle, "second");
    assert_eq!(registry.handoffs.len(), 1);
}

#[test]
fn registry_remembers_patch_handoffs_by_worktree_pair() {
    let repo = PathBuf::from("/repo/hz");
    let local = WorktreeTarget {
        name: "local".to_owned(),
        path: repo.clone(),
    };
    let detached = WorktreeTarget {
        name: "13n3".to_owned(),
        path: PathBuf::from("/worktrees/13n3"),
    };
    let mut registry = Registry::default();

    registry
        .remember_patch_handoff(&repo, &detached, &local, "first".to_owned())
        .unwrap();
    registry
        .remember_patch_handoff(&repo, &local, &detached, "second".to_owned())
        .unwrap();

    let link = registry
        .patch_handoff_link(&repo, &detached.path, &local.path)
        .unwrap();
    assert_eq!(link.patch_hash, "second");
    assert_eq!(registry.patch_handoffs.len(), 1);
}

#[test]
fn registry_finds_latest_patch_handoff_for_path() {
    let repo = PathBuf::from("/repo/hz");
    let local = PathBuf::from("/repo/hz");
    let first = PathBuf::from("/worktrees/first");
    let second = PathBuf::from("/worktrees/second");
    let registry = Registry {
        entries: Vec::new(),
        handoffs: Vec::new(),
        patch_handoffs: vec![
            PatchHandoffLink {
                repo: repo.clone(),
                left_path: first,
                left_handle: "first".to_owned(),
                right_path: local.clone(),
                right_handle: "local".to_owned(),
                patch_hash: "older".to_owned(),
                updated_at_unix: 1,
            },
            PatchHandoffLink {
                repo: repo.clone(),
                left_path: local.clone(),
                left_handle: "local".to_owned(),
                right_path: second,
                right_handle: "second".to_owned(),
                patch_hash: "newer".to_owned(),
                updated_at_unix: 2,
            },
        ],
    };

    let link = registry
        .latest_patch_handoff_for_path(&repo, &local)
        .unwrap();

    assert_eq!(link.patch_hash, "newer");
}

#[test]
fn handoff_source_branch_must_match_requested_branch() {
    let error = validate_handoff_source_branch(
        &PathBuf::from("/worktrees/current"),
        Some("feature/other"),
        "feature/ui",
    )
    .unwrap_err();

    assert_eq!(
        error.to_string(),
        "/worktrees/current is on branch feature/other, not feature/ui"
    );
    assert!(
        validate_handoff_source_branch(
            &PathBuf::from("/worktrees/current"),
            Some("feature/ui"),
            "feature/ui"
        )
        .is_ok()
    );
    assert!(
        validate_handoff_source_branch(&PathBuf::from("/worktrees/current"), None, "feature/ui")
            .is_ok()
    );
}

#[test]
fn branch_handoff_rollback_continues_after_restore_failure() {
    let test_dir = test_dir("hz-worktree-branch-rollback-continue-test");
    let repo = test_dir.join("repo");
    init_committed_repo(&repo);
    git(["branch", "-m", "main"], &repo);
    let main_checkout = GitCheckout::current(&repo).unwrap();
    git(["switch", "-q", "-c", "other"], &repo);

    let mut applied = AppliedBranchHandoff::default();
    applied.push(&repo, main_checkout);
    applied.push(
        &test_dir.join("missing-worktree"),
        GitCheckout {
            branch: Some("main".to_owned()),
            head: "HEAD".to_owned(),
        },
    );

    let error = applied.rollback().unwrap_err();

    assert!(
        error
            .to_string()
            .contains("failed to restore one or more worktrees"),
        "{error}"
    );
    assert_eq!(
        hz_git::current_branch(&repo).unwrap().as_deref(),
        Some("main")
    );

    fs::remove_dir_all(test_dir).expect("test directory should be removed");
}

#[test]
fn local_to_worktree_branch_handoff_rolls_back_on_save_failure() {
    let test_dir = test_dir("hz-worktree-local-branch-rollback-test");
    let repo = test_dir.join("repo");
    let destination = test_dir.join("destination");
    init_committed_repo(&repo);
    git(["branch", "-m", "main"], &repo);
    git(["switch", "-q", "-c", "feature"], &repo);
    git(
        [
            "worktree",
            "add",
            "-q",
            "--detach",
            destination.to_str().unwrap(),
            "main",
        ],
        &repo,
    );
    let destination_head = hz_git::current_head(&destination).unwrap();
    let local_checkout = GitCheckout::current(&repo).unwrap();
    let destination_checkout = GitCheckout::current(&destination).unwrap();

    let applied = apply_local_to_worktree_branch_handoff(
        &repo,
        &destination,
        "feature",
        None,
        local_checkout,
        destination_checkout,
    )
    .unwrap();
    let error =
        rollback_saved_branch_handoff(applied, HzError::Usage("registry save failed".to_owned()));

    assert_eq!(error.to_string(), "registry save failed");
    assert_eq!(
        hz_git::current_branch(&repo).unwrap().as_deref(),
        Some("feature")
    );
    assert_eq!(hz_git::current_branch(&destination).unwrap(), None);
    assert_eq!(
        hz_git::current_head(&destination).unwrap(),
        destination_head
    );

    fs::remove_dir_all(test_dir).expect("test directory should be removed");
}

#[test]
fn local_to_worktree_branch_handoff_rolls_back_when_registry_save_fails() {
    let test_dir = test_dir("hz-worktree-local-branch-save-failure-test");
    let repo = test_dir.join("repo");
    let destination = test_dir.join("destination");
    init_committed_repo(&repo);
    git(["branch", "-m", "main"], &repo);
    git(["switch", "-q", "-c", "feature"], &repo);
    git(
        [
            "worktree",
            "add",
            "-q",
            "--detach",
            destination.to_str().unwrap(),
            "main",
        ],
        &repo,
    );
    let destination_head = hz_git::current_head(&destination).unwrap();
    let destination_entry = WorktreeEntry {
        id: "destination".to_owned(),
        handle: "destination".to_owned(),
        repo: repo.clone(),
        path: destination.clone(),
        branch: None,
        base: None,
        source: WorktreeSource::Git,
        created_at_unix: 0,
        modified_at_unix: 0,
        status: WorktreeStatus::Unknown,
    };
    let mut registry = Registry::default();
    registry
        .remember_handoff(
            &repo,
            "feature",
            &destination,
            "destination",
            Some("main".to_owned()),
        )
        .unwrap();
    let blocked_parent = test_dir.join("blocked-registry-parent");
    fs::write(&blocked_parent, "not a directory")
        .expect("blocked registry parent should be written");
    let _registry_path_override =
        RegistryPathOverrideGuard::set(blocked_parent.join("registry.json"));

    let error = handoff_local_to_worktree(
        &mut registry,
        repo.clone(),
        repo.clone(),
        "feature".to_owned(),
        Some(destination_entry),
    )
    .unwrap_err();

    assert!(matches!(&error, HzError::Io(_)), "{error}");
    assert_eq!(
        hz_git::current_branch(&repo).unwrap().as_deref(),
        Some("feature")
    );
    assert_eq!(hz_git::current_branch(&destination).unwrap(), None);
    assert_eq!(
        hz_git::current_head(&destination).unwrap(),
        destination_head
    );
    let link = registry.handoff_link(&repo, "feature").unwrap();
    assert!(same_path(&link.path, &destination));
    assert_eq!(link.local_restore_branch.as_deref(), Some("main"));

    fs::remove_dir_all(test_dir).expect("test directory should be removed");
}

#[test]
fn worktree_to_local_branch_handoff_rolls_back_on_save_failure() {
    let test_dir = test_dir("hz-worktree-worktree-branch-rollback-test");
    let repo = test_dir.join("repo");
    let source = test_dir.join("source");
    init_committed_repo(&repo);
    git(["branch", "-m", "main"], &repo);
    git(["branch", "feature"], &repo);
    git(
        ["worktree", "add", "-q", source.to_str().unwrap(), "feature"],
        &repo,
    );
    let source_checkout = GitCheckout::current(&source).unwrap();
    let local_checkout = GitCheckout::current(&repo).unwrap();

    let applied = apply_worktree_to_local_branch_handoff(
        &source,
        &repo,
        "feature",
        source_checkout,
        local_checkout,
    )
    .unwrap();
    let error =
        rollback_saved_branch_handoff(applied, HzError::Usage("registry save failed".to_owned()));

    assert_eq!(error.to_string(), "registry save failed");
    assert_eq!(
        hz_git::current_branch(&repo).unwrap().as_deref(),
        Some("main")
    );
    assert_eq!(
        hz_git::current_branch(&source).unwrap().as_deref(),
        Some("feature")
    );

    fs::remove_dir_all(test_dir).expect("test directory should be removed");
}

#[test]
fn local_handoff_target_can_match_detached_codex_worktree_handle() {
    let repo = PathBuf::from("/repo/hz");
    let entry = git_entry(
        &repo,
        hz_git::GitWorktree {
            path: PathBuf::from("/Users/dev/.codex/worktrees/708e/hz"),
            branch: None,
        },
    );

    let destination = find_target_entry(vec![entry], &repo, "708e").unwrap();

    assert_eq!(destination.handle, "708e");
    assert_eq!(destination.branch, None);
    assert_eq!(
        destination.path,
        PathBuf::from("/Users/dev/.codex/worktrees/708e/hz")
    );
}

#[test]
fn worktree_entries_sort_newest_first_with_handle_tiebreaker() {
    let repo = PathBuf::from("/repo/hz");
    let mut entries = vec![
        WorktreeEntry {
            created_at_unix: 20,
            modified_at_unix: 0,
            status: WorktreeStatus::Unknown,
            ..test_entry(&repo, "zeta".to_owned())
        },
        WorktreeEntry {
            created_at_unix: 30,
            modified_at_unix: 0,
            status: WorktreeStatus::Unknown,
            ..test_entry(&repo, "beta".to_owned())
        },
        WorktreeEntry {
            created_at_unix: 30,
            modified_at_unix: 0,
            status: WorktreeStatus::Unknown,
            ..test_entry(&repo, "alpha".to_owned())
        },
    ];

    sort_worktree_entries(&mut entries);

    let handles: Vec<_> = entries.iter().map(|entry| entry.handle.as_str()).collect();
    assert_eq!(handles, vec!["alpha", "beta", "zeta"]);
}

#[test]
fn registry_temp_paths_are_unique_and_adjacent() {
    let registry = PathBuf::from("/config/hz/registry.json");
    let first = registry_temp_path(&registry).unwrap();
    let second = registry_temp_path(&registry).unwrap();

    assert_ne!(first, second);
    assert_eq!(first.parent(), registry.parent());
    assert!(
        first
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with(".registry.json.")
    );
}

#[test]
fn registry_lock_path_is_adjacent_to_registry() {
    let registry = PathBuf::from("/config/hz/registry.json");

    assert_eq!(
        registry_lock_path(&registry).unwrap(),
        PathBuf::from("/config/hz/registry.json.lock")
    );
}

#[test]
fn registry_lock_file_is_exclusive() {
    let lock_path =
        env::temp_dir().join(format!("hz-registry-lock-{}.lock", new_uuid_v4().unwrap()));
    let first = open_registry_lock_file(&lock_path).unwrap();
    lock_registry_file(&first).unwrap();
    let second = open_registry_lock_file(&lock_path).unwrap();

    let blocked = fs2::FileExt::try_lock_exclusive(&second).unwrap_err();
    assert_eq!(blocked.kind(), fs2::lock_contended_error().kind());

    unlock_registry_file(&first).unwrap();
    fs2::FileExt::lock_exclusive(&second).unwrap();
    fs2::FileExt::unlock(&second).unwrap();
    fs::remove_file(lock_path).unwrap();
}

#[test]
fn registry_lock_acquire_rejects_same_thread_reentry() {
    let lock_path = env::temp_dir().join(format!(
        "hz-registry-reentry-{}.lock",
        new_uuid_v4().unwrap()
    ));
    let first = RegistryLock::acquire_path(&lock_path).unwrap();

    let second = RegistryLock::acquire_path(&lock_path);
    assert!(second.is_err());
    assert_eq!(
        second.err().unwrap().to_string(),
        "registry lock is already held by this thread"
    );

    drop(first);
    fs::remove_file(lock_path).unwrap();
}

#[test]
fn registry_lock_for_git_side_effect_reuses_current_lock() {
    let lock_path = env::temp_dir().join(format!(
        "hz-registry-git-side-effect-{}.lock",
        new_uuid_v4().unwrap()
    ));
    let first = RegistryLock::acquire_path(&lock_path).unwrap();

    let result = run_with_registry_lock_for_git_side_effect(|| Ok("removed"));

    assert_eq!(result.unwrap(), "removed");
    drop(first);
    fs::remove_file(lock_path).unwrap();
}

#[test]
fn registry_path_ignores_empty_xdg_config_home() {
    assert_eq!(
        registry_path_from_env(Some(PathBuf::from("/home/user")), Some(PathBuf::new())).unwrap(),
        PathBuf::from("/home/user/.config/hz/registry.json")
    );
    assert_eq!(
        registry_path_from_env(
            Some(PathBuf::from("/home/user")),
            Some(PathBuf::from("/tmp/config")),
        )
        .unwrap(),
        PathBuf::from("/tmp/config/hz/registry.json")
    );
}

#[test]
fn registry_path_does_not_fall_back_to_empty_home() {
    assert_eq!(
        registry_path_from_env(Some(PathBuf::new()), Some(PathBuf::from("/tmp/config")),).unwrap(),
        PathBuf::from("/tmp/config/hz/registry.json")
    );
    assert!(registry_path_from_env(Some(PathBuf::new()), Some(PathBuf::new())).is_err());
}

#[test]
fn git_worktree_handle_uses_parent_when_path_leaf_is_repo_name() {
    let handle = git_worktree_handle(
        &PathBuf::from("/repo/hz"),
        &hz_git::GitWorktree {
            path: PathBuf::from("/Users/dev/.codex/worktrees/bd16/hz"),
            branch: None,
        },
    );

    assert_eq!(handle, "bd16");
}

#[test]
fn git_worktree_handle_uses_path_leaf_when_it_is_specific() {
    let handle = git_worktree_handle(
        &PathBuf::from("/repo/hz"),
        &hz_git::GitWorktree {
            path: PathBuf::from("/repo/hz-feature"),
            branch: None,
        },
    );

    assert_eq!(handle, "hz-feature");
}

#[test]
fn git_worktree_handle_keeps_tool_directory_when_branch_exists() {
    let entry = git_entry(
        &PathBuf::from("/repo/hz"),
        hz_git::GitWorktree {
            path: PathBuf::from("/Users/dev/.codex/worktrees/bd16/hz"),
            branch: Some("feature/list".to_owned()),
        },
    );

    assert_eq!(entry.handle, "bd16");
    assert_eq!(entry.branch.as_deref(), Some("feature/list"));
    assert!(matches_target(&entry, "bd16"));
    assert!(matches_target(&entry, "feature/list"));
}

#[test]
fn git_worktree_merge_refreshes_registered_branch_and_skips_registered_path() {
    let repo = PathBuf::from("/repo/hz");
    let mut entries = vec![WorktreeEntry {
        id: "managed-id".to_owned(),
        handle: "managed".to_owned(),
        repo: repo.clone(),
        path: PathBuf::from("/worktrees/managed"),
        branch: None,
        base: None,
        source: WorktreeSource::Managed,
        created_at_unix: 0,
        modified_at_unix: 0,
        status: WorktreeStatus::Unknown,
    }];

    add_git_worktrees(
        &mut entries,
        &repo,
        vec![
            hz_git::GitWorktree {
                path: repo.clone(),
                branch: Some("main".to_owned()),
            },
            hz_git::GitWorktree {
                path: PathBuf::from("/worktrees/managed"),
                branch: Some("helloworld".to_owned()),
            },
            hz_git::GitWorktree {
                path: PathBuf::from("/Users/dev/.codex/worktrees/bd16/hz"),
                branch: None,
            },
        ],
    );

    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].branch.as_deref(), Some("helloworld"));
    assert_eq!(entries[1].handle, "bd16");
    assert_eq!(entries[1].source, WorktreeSource::Git);

    let entry = find_target_entry(entries, &repo, "helloworld").unwrap();
    assert_eq!(entry.handle, "managed");
    assert_eq!(entry.source, WorktreeSource::Managed);
}

#[test]
fn git_worktree_merge_skips_primary_when_repo_is_linked_worktree() {
    let repo = PathBuf::from("/Users/dev/.codex/worktrees/current/hz");
    let mut entries = Vec::new();

    add_git_worktrees(
        &mut entries,
        &repo,
        vec![
            hz_git::GitWorktree {
                path: PathBuf::from("/repo/hz"),
                branch: Some("main".to_owned()),
            },
            hz_git::GitWorktree {
                path: repo.clone(),
                branch: Some("feature/current".to_owned()),
            },
            hz_git::GitWorktree {
                path: PathBuf::from("/Users/dev/.codex/worktrees/other/hz"),
                branch: Some("feature/other".to_owned()),
            },
        ],
    );

    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].branch.as_deref(), Some("feature/other"));
}

fn test_entry(repo: &Path, handle: String) -> WorktreeEntry {
    WorktreeEntry {
        id: handle.clone(),
        handle: handle.clone(),
        repo: repo.to_path_buf(),
        path: PathBuf::from("/worktrees").join(&handle),
        branch: Some(handle),
        base: None,
        source: WorktreeSource::Managed,
        created_at_unix: 0,
        modified_at_unix: 0,
        status: WorktreeStatus::Unknown,
    }
}

fn detached_test_entry(repo: &Path, handle: &str, created_at_unix: u64) -> WorktreeEntry {
    WorktreeEntry {
        id: handle.to_owned(),
        handle: handle.to_owned(),
        repo: repo.to_path_buf(),
        path: PathBuf::from("/worktrees").join(handle),
        branch: None,
        base: None,
        source: WorktreeSource::Managed,
        created_at_unix,
        modified_at_unix: 0,
        status: WorktreeStatus::Unknown,
    }
}

fn git_detached(entry: &WorktreeEntry) -> hz_git::GitWorktree {
    hz_git::GitWorktree {
        path: entry.path.clone(),
        branch: None,
    }
}

fn git_worktree_listed(repo: &Path, path: &Path) -> bool {
    hz_git::list_worktrees(repo)
        .unwrap()
        .into_iter()
        .any(|worktree| same_path(&worktree.path, path))
}

fn test_dir(prefix: &str) -> PathBuf {
    let test_dir = env::temp_dir().join(format!(
        "{}-{}",
        prefix,
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after unix epoch")
            .as_nanos()
    ));
    fs::create_dir_all(&test_dir).expect("test directory should be created");
    test_dir
}

fn init_committed_repo(repo: &Path) {
    let parent = repo.parent().expect("repo should have parent");
    git(["init", "-q", repo.to_str().unwrap()], parent);
    git(["config", "user.email", "test@example.com"], repo);
    git(["config", "user.name", "Test"], repo);
    fs::write(repo.join("file.txt"), "base\n").expect("tracked file should be written");
    git(["add", "file.txt"], repo);
    git(["commit", "-q", "-m", "init"], repo);
}

fn git<const N: usize>(args: [&str; N], cwd: &Path) {
    let output = Command::new("git")
        .current_dir(cwd)
        .args(args)
        .output()
        .expect("git should run");
    assert!(
        output.status.success(),
        "git failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
