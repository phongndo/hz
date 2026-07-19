use std::{
    fs,
    io::{ErrorKind, Read, Write},
    path::{Path, PathBuf},
};

use crate::{Error, Result};

pub const FILE_NAME: &str = ".hz-workspace";

pub fn path(workspace: &Path) -> PathBuf {
    workspace.join(FILE_NAME)
}

pub fn write(workspace: &Path, id: &str) -> Result<()> {
    ensure_workspace_directory(workspace)?;
    ensure_safe_marker_if_present(workspace)?;
    atomic_write(&path(workspace), format!("{id}\n").as_bytes(), None)
}

fn atomic_write(
    destination: &Path,
    contents: &[u8],
    permissions: Option<fs::Permissions>,
) -> Result<()> {
    let parent = destination.parent().ok_or_else(|| {
        Error::Path(format!(
            "file has no parent directory: {}",
            destination.display()
        ))
    })?;
    let (temporary, mut file) = create_temporary_file(parent)?;
    let written = (|| -> std::io::Result<()> {
        file.write_all(contents)?;
        if let Some(permissions) = permissions {
            file.set_permissions(permissions)?;
        }
        Ok(())
    })();
    drop(file);
    if let Err(error) = written {
        let _ = fs::remove_file(&temporary);
        return Err(error.into());
    }
    if let Err(error) = replace(&temporary, destination) {
        let _ = fs::remove_file(&temporary);
        return Err(error.into());
    }
    Ok(())
}

fn create_temporary_file(parent: &Path) -> Result<(PathBuf, fs::File)> {
    for _ in 0..16 {
        let mut random = [0_u8; 16];
        getrandom::getrandom(&mut random).map_err(|error| {
            std::io::Error::other(format!("failed to generate a temporary file name: {error}"))
        })?;
        let nonce = u128::from_le_bytes(random);
        let temporary = parent.join(format!("{FILE_NAME}.{nonce:032x}.tmp"));
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        match options.open(&temporary) {
            Ok(file) => return Ok((temporary, file)),
            Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
    Err(std::io::Error::new(
        ErrorKind::AlreadyExists,
        format!(
            "failed to create a unique temporary marker in {}",
            parent.display()
        ),
    )
    .into())
}

#[cfg(not(windows))]
fn replace(source: &Path, destination: &Path) -> std::io::Result<()> {
    fs::rename(source, destination)
}

#[cfg(windows)]
fn replace(source: &Path, destination: &Path) -> std::io::Result<()> {
    match fs::remove_file(destination) {
        Ok(()) => {}
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    fs::rename(source, destination)
}

pub fn read(workspace: &Path) -> Result<Option<String>> {
    match symlink_metadata_if_exists(workspace)? {
        Some(metadata) if metadata.is_dir() && !metadata_is_link(&metadata) => {}
        Some(_) => return Err(Error::InvalidMarker(workspace.to_path_buf())),
        None => return Ok(None),
    }
    let marker_path = path(workspace);
    let Some(metadata) = symlink_metadata_if_exists(&marker_path)? else {
        return Ok(None);
    };
    if !metadata.is_file() || metadata_is_link(&metadata) {
        return Err(Error::InvalidMarker(workspace.to_path_buf()));
    }
    let contents = match read_to_string_no_follow(&marker_path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let id = contents.trim();
    if id.is_empty() || id.lines().count() != 1 || ulid::Ulid::from_string(id).is_err() {
        return Err(Error::InvalidMarker(workspace.to_path_buf()));
    }
    Ok(Some(id.to_owned()))
}

fn read_to_string_no_follow(path: &Path) -> std::io::Result<String> {
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = options.open(path)?;
    let mut contents = String::new();
    file.read_to_string(&mut contents)?;
    Ok(contents)
}

fn ensure_workspace_directory(workspace: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(workspace)?;
    if !metadata.is_dir() || metadata_is_link(&metadata) {
        return Err(Error::InvalidMarker(workspace.to_path_buf()));
    }
    Ok(())
}

pub(crate) fn ensure_real_workspace_path(workspace: &Path) -> Result<()> {
    ensure_workspace_directory(workspace)?;
    for ancestor in workspace.ancestors().skip(1) {
        if ancestor.as_os_str().is_empty() {
            continue;
        }
        if metadata_is_link(&fs::symlink_metadata(ancestor)?) {
            return Err(Error::InvalidMarker(workspace.to_path_buf()));
        }
    }
    Ok(())
}

fn ensure_safe_marker_if_present(workspace: &Path) -> Result<()> {
    let marker_path = path(workspace);
    let Some(metadata) = symlink_metadata_if_exists(&marker_path)? else {
        return Ok(());
    };
    if !metadata.is_file() || metadata_is_link(&metadata) {
        return Err(Error::InvalidMarker(workspace.to_path_buf()));
    }
    Ok(())
}

#[cfg(not(windows))]
fn metadata_is_link(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

#[cfg(windows)]
fn metadata_is_link(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;

    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    metadata.file_type().is_symlink()
        || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

pub fn verify(workspace: &Path, expected_id: &str) -> Result<()> {
    ensure_real_workspace_path(workspace)?;
    if read(workspace)?.as_deref() == Some(expected_id) {
        Ok(())
    } else {
        Err(Error::MarkerMismatch(workspace.to_path_buf()))
    }
}

pub fn remove(workspace: &Path) -> Result<()> {
    match ensure_workspace_directory(workspace) {
        Ok(()) => {}
        Err(Error::Io(error)) if error.kind() == ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    }
    match fs::remove_file(path(workspace)) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

const GIT_EXCLUDE_BLOCK: &[u8] = b"# Keep Hz workspace metadata out of source control\n\
.hz-workspace\n\
.hz-workspaces/\n";
const HG_IGNORE_CONTENTS: &[u8] = b"syntax: regexp\n\
(?:^|/)\\.hz-workspace$\n\
(?:^|/)\\.hz-workspaces(?:/|$)\n";
const HG_CONFIG_BLOCK: &[u8] = b"# Keep Hz workspace metadata out of source control\n\
[ui]\nignore.hz-workspace = .hg/hz-workspace.ignore\n";

pub(crate) fn protect_from_source_control(workspace: &Path) -> Result<()> {
    ensure_real_workspace_path(workspace)?;
    for directory in workspace.ancestors() {
        protect_from_git(directory)?;
        protect_from_mercurial(directory)?;
    }
    Ok(())
}

fn protect_from_git(repository: &Path) -> Result<()> {
    let metadata_path = repository.join(".git");
    let Some(metadata) = symlink_metadata_if_exists(&metadata_path)? else {
        return Ok(());
    };
    if metadata.file_type().is_symlink() {
        return Err(unsafe_source_control_path(&metadata_path));
    }
    if metadata.is_dir() {
        let info = metadata_path.join("info");
        ensure_real_directory(&info)?;
        return append_block(&info.join("exclude"), GIT_EXCLUDE_BLOCK);
    }
    if metadata.is_file() {
        // Linked worktrees and submodules use a .git file that points at their
        // real Git metadata. Git resolves info/exclude through commondir for a
        // linked worktree, so update that metadata rather than dirtying the
        // worktree with a .gitignore change.
        let git_directory = git_directory_from_file(&metadata_path)?;
        let common_directory = git_common_directory(&git_directory)?;
        let info = common_directory.join("info");
        ensure_real_directory(&info)?;
        return append_block(&info.join("exclude"), GIT_EXCLUDE_BLOCK);
    }
    Err(unsafe_source_control_path(&metadata_path))
}

fn git_directory_from_file(metadata_path: &Path) -> Result<PathBuf> {
    let contents = read_regular_file(metadata_path)?
        .ok_or_else(|| unsafe_source_control_path(metadata_path))?
        .0;
    let pointer = contents
        .strip_prefix(b"gitdir: ")
        .and_then(single_line_git_path)
        .ok_or_else(|| unsafe_source_control_path(metadata_path))?;
    canonical_git_directory(metadata_path.parent().unwrap_or(Path::new("")), pointer)
}

fn git_common_directory(git_directory: &Path) -> Result<PathBuf> {
    let commondir = git_directory.join("commondir");
    let Some((contents, _)) = read_regular_file(&commondir)? else {
        return Ok(git_directory.to_path_buf());
    };
    let pointer =
        single_line_git_path(&contents).ok_or_else(|| unsafe_source_control_path(&commondir))?;
    canonical_git_directory(git_directory, pointer)
}

fn single_line_git_path(contents: &[u8]) -> Option<&[u8]> {
    let contents = contents.strip_suffix(b"\n").unwrap_or(contents);
    let contents = contents.strip_suffix(b"\r").unwrap_or(contents);
    (!contents.is_empty()
        && !contents.contains(&b'\n')
        && !contents.contains(&b'\r')
        && !contents.contains(&0))
    .then_some(contents)
}

fn canonical_git_directory(base: &Path, encoded: &[u8]) -> Result<PathBuf> {
    let path = git_path_from_bytes(encoded)?;
    let path = if path.is_absolute() {
        path
    } else {
        base.join(path)
    };
    let path = fs::canonicalize(path)?;
    let metadata = fs::symlink_metadata(&path)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(unsafe_source_control_path(&path));
    }
    Ok(path)
}

#[cfg(unix)]
fn git_path_from_bytes(encoded: &[u8]) -> Result<PathBuf> {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    Ok(PathBuf::from(OsString::from_vec(encoded.to_vec())))
}

#[cfg(not(unix))]
fn git_path_from_bytes(encoded: &[u8]) -> Result<PathBuf> {
    String::from_utf8(encoded.to_vec())
        .map(PathBuf::from)
        .map_err(|_| Error::Path("Git metadata path is not valid UTF-8".to_owned()))
}

fn protect_from_mercurial(repository: &Path) -> Result<()> {
    let metadata_path = repository.join(".hg");
    let Some(metadata) = symlink_metadata_if_exists(&metadata_path)? else {
        return Ok(());
    };
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(unsafe_source_control_path(&metadata_path));
    }
    write_if_different(
        &metadata_path.join("hz-workspace.ignore"),
        HG_IGNORE_CONTENTS,
    )?;
    append_block(&metadata_path.join("hgrc"), HG_CONFIG_BLOCK)
}

fn append_block(path: &Path, block: &[u8]) -> Result<()> {
    let existing = read_regular_file(path)?;
    let (mut contents, permissions) = match existing {
        Some((contents, _)) if contents.ends_with(block) => return Ok(()),
        Some(existing) => existing,
        None => (Vec::new(), None),
    };
    if !contents.is_empty() && !contents.ends_with(b"\n") {
        contents.push(b'\n');
    }
    contents.extend_from_slice(block);
    atomic_write(path, &contents, permissions)
}

fn write_if_different(path: &Path, expected: &[u8]) -> Result<()> {
    let existing = read_regular_file(path)?;
    let permissions = match existing {
        Some((contents, _)) if contents == expected => return Ok(()),
        Some((_, permissions)) => permissions,
        None => None,
    };
    atomic_write(path, expected, permissions)
}

fn read_regular_file(path: &Path) -> Result<Option<(Vec<u8>, Option<fs::Permissions>)>> {
    let Some(metadata) = symlink_metadata_if_exists(path)? else {
        return Ok(None);
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(unsafe_source_control_path(path));
    }
    Ok(Some((fs::read(path)?, Some(metadata.permissions()))))
}

fn ensure_real_directory(path: &Path) -> Result<()> {
    match symlink_metadata_if_exists(path)? {
        Some(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => Ok(()),
        Some(_) => Err(unsafe_source_control_path(path)),
        None => match fs::create_dir(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {
                match symlink_metadata_if_exists(path)? {
                    Some(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
                        Ok(())
                    }
                    _ => Err(unsafe_source_control_path(path)),
                }
            }
            Err(error) => Err(error.into()),
        },
    }
}

fn symlink_metadata_if_exists(path: &Path) -> Result<Option<fs::Metadata>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => Ok(Some(metadata)),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn unsafe_source_control_path(path: &Path) -> Error {
    Error::Path(format!(
        "refusing to modify unsafe source-control metadata: {}",
        path.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn write_replaces_an_existing_marker() {
        let temp = TempDir::new().unwrap();
        let first = ulid::Ulid::new().to_string();
        let second = ulid::Ulid::new().to_string();

        write(temp.path(), &first).unwrap();
        write(temp.path(), &second).unwrap();

        assert_eq!(read(temp.path()).unwrap().as_deref(), Some(second.as_str()));
    }

    #[cfg(unix)]
    #[test]
    fn write_does_not_follow_a_precreated_predictable_temp_symlink() {
        let temp = TempDir::new().unwrap();
        let workspace = temp.path().join("workspace");
        let target = temp.path().join("target.txt");
        fs::create_dir(&workspace).unwrap();
        fs::write(&target, "do not truncate").unwrap();
        let id = ulid::Ulid::new().to_string();
        let legacy_temporary = workspace.join(format!(".{FILE_NAME}.{id}.tmp"));
        std::os::unix::fs::symlink(&target, &legacy_temporary).unwrap();

        write(&workspace, &id).unwrap();

        assert_eq!(fs::read_to_string(&target).unwrap(), "do not truncate");
        assert_eq!(read(&workspace).unwrap(), Some(id));
        assert_eq!(fs::read_link(legacy_temporary).unwrap(), target);
    }

    #[cfg(unix)]
    #[test]
    fn read_rejects_a_symlinked_workspace_directory() {
        let temp = TempDir::new().unwrap();
        let target = temp.path().join("target");
        let workspace = temp.path().join("workspace");
        fs::create_dir(&target).unwrap();
        let id = ulid::Ulid::new().to_string();
        write(&target, &id).unwrap();
        std::os::unix::fs::symlink(&target, &workspace).unwrap();

        assert!(matches!(
            read(&workspace),
            Err(Error::InvalidMarker(path)) if path == workspace
        ));
        assert_eq!(read(&target).unwrap(), Some(id));
    }

    #[cfg(unix)]
    #[test]
    fn read_rejects_a_symlinked_marker() {
        let temp = TempDir::new().unwrap();
        let workspace = temp.path().join("workspace");
        let target = temp.path().join("target-marker");
        fs::create_dir(&workspace).unwrap();
        let id = ulid::Ulid::new().to_string();
        fs::write(&target, format!("{id}\n")).unwrap();
        std::os::unix::fs::symlink(&target, path(&workspace)).unwrap();

        assert!(matches!(
            read(&workspace),
            Err(Error::InvalidMarker(path)) if path == workspace
        ));
        assert_eq!(fs::read_to_string(target).unwrap(), format!("{id}\n"));
    }

    #[cfg(unix)]
    #[test]
    fn verification_rejects_a_symlinked_workspace_ancestor() {
        let temp = TempDir::new().unwrap();
        let temp = fs::canonicalize(temp.path()).unwrap();
        let real_parent = temp.join("real-parent");
        let real_workspace = real_parent.join("workspace");
        let linked_parent = temp.join("linked-parent");
        fs::create_dir_all(&real_workspace).unwrap();
        let id = ulid::Ulid::new().to_string();
        write(&real_workspace, &id).unwrap();
        std::os::unix::fs::symlink(&real_parent, &linked_parent).unwrap();
        let linked_workspace = linked_parent.join("workspace");

        assert_eq!(read(&linked_workspace).unwrap(), Some(id.clone()));
        assert!(matches!(
            verify(&linked_workspace, &id),
            Err(Error::InvalidMarker(path)) if path == linked_workspace
        ));
    }

    #[test]
    fn linked_git_worktree_protection_does_not_dirty_the_worktree() {
        use std::process::Command;

        if Command::new("git").arg("--version").output().is_err() {
            return;
        }
        let temp = TempDir::new().unwrap();
        let repository = temp.path().join("repository");
        let workspace = temp.path().join("workspace");
        let run_git = |at: &Path, args: &[&str]| {
            let output = Command::new("git")
                .arg("-C")
                .arg(at)
                .args(args)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "git {} failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&output.stderr)
            );
            output
        };

        let output = Command::new("git")
            .arg("init")
            .arg(&repository)
            .output()
            .unwrap();
        assert!(output.status.success());
        run_git(&repository, &["config", "user.name", "Hz Test"]);
        run_git(&repository, &["config", "user.email", "hz@example.invalid"]);
        fs::write(repository.join("tracked"), "tracked\n").unwrap();
        run_git(&repository, &["add", "tracked"]);
        run_git(
            &repository,
            &["-c", "commit.gpgsign=false", "commit", "-m", "initial"],
        );
        let workspace_arg = workspace.to_str().unwrap();
        run_git(&repository, &["worktree", "add", "--detach", workspace_arg]);
        let workspace = fs::canonicalize(workspace).unwrap();

        protect_from_git(&workspace).unwrap();
        write(&workspace, &ulid::Ulid::new().to_string()).unwrap();

        assert!(!workspace.join(".gitignore").exists());
        assert!(
            fs::read(repository.join(".git/info/exclude"))
                .unwrap()
                .ends_with(GIT_EXCLUDE_BLOCK)
        );
        let status = run_git(
            &workspace,
            &["status", "--porcelain=v1", "--untracked-files=all"],
        );
        assert!(status.stdout.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn source_control_protection_refuses_to_follow_an_exclude_symlink() {
        let temp = TempDir::new().unwrap();
        let workspace = temp.path().join("workspace");
        let info = workspace.join(".git/info");
        let target = temp.path().join("target.txt");
        fs::create_dir_all(&info).unwrap();
        fs::write(&target, "do not modify").unwrap();
        std::os::unix::fs::symlink(&target, info.join("exclude")).unwrap();
        let workspace = fs::canonicalize(workspace).unwrap();

        assert!(matches!(
            protect_from_source_control(&workspace),
            Err(Error::Path(_))
        ));
        assert_eq!(fs::read_to_string(target).unwrap(), "do not modify");
    }
}
