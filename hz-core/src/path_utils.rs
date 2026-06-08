use std::{
    fs,
    path::{Component, Path, PathBuf},
};

/// Returns true when `path` is under `root`, using lexical normalization before
/// falling back to filesystem canonicalization for existing paths.
pub fn path_is_inside(path: &Path, root: &Path) -> bool {
    if path.starts_with(root) {
        return true;
    }

    let normalized_path = normalize_lexically(path);
    let normalized_root = normalize_lexically(root);
    if normalized_path.starts_with(&normalized_root) {
        return true;
    }

    fs::canonicalize(path)
        .ok()
        .zip(fs::canonicalize(root).ok())
        .is_some_and(|(path, root)| path.starts_with(root))
}

/// Normalizes `.` and `..` path components without touching the filesystem.
pub fn normalize_lexically(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();

    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => match normalized.components().next_back() {
                Some(Component::Normal(_)) => {
                    normalized.pop();
                }
                Some(Component::ParentDir) | None => normalized.push(component.as_os_str()),
                Some(Component::Prefix(_)) | Some(Component::RootDir) => {}
                Some(Component::CurDir) => unreachable!("normalized paths never contain curdir"),
            },
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                normalized.push(component.as_os_str());
            }
        }
    }

    if normalized.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        normalized
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        env,
        time::{SystemTime, UNIX_EPOCH},
    };

    #[test]
    fn path_is_inside_matches_lexically_normalized_roots() {
        assert!(path_is_inside(
            Path::new("/repo/agent-worktrees/entry"),
            Path::new("/repo/hz/../agent-worktrees"),
        ));
    }

    #[cfg(unix)]
    #[test]
    fn path_is_inside_matches_canonicalized_paths() {
        let test_dir = test_dir("hz-core-path-utils-test");
        let root = test_dir.join("root");
        let child = root.join("child");
        fs::create_dir_all(&child).expect("child directory should be created");
        let link = test_dir.join("link");
        std::os::unix::fs::symlink(&root, &link).expect("symlink should be created");

        assert!(!link.join("child").starts_with(&root));
        assert!(path_is_inside(&link.join("child"), &root));

        fs::remove_dir_all(test_dir).expect("test directory should be removed");
    }

    fn test_dir(prefix: &str) -> PathBuf {
        let test_dir = env::temp_dir().join(format!(
            "{prefix}-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time should be after unix epoch")
                .as_nanos()
        ));
        fs::create_dir_all(&test_dir).expect("test directory should be created");
        test_dir
    }
}
