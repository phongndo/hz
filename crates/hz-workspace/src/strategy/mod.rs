// Native COW strategy implementations are adapted from anomalyco/rift's
// MIT-licensed filesystem layer and integrated with Hz's workspace model.
use crate::{CopyMode, Error, InitProgress, Result, filter::CopyFilter};
use std::{collections::HashMap, fs, path::Path};

#[cfg(target_os = "macos")]
mod apfs;
#[cfg(target_os = "linux")]
mod btrfs;
#[cfg(target_os = "linux")]
mod linux;
mod mount;
#[cfg(target_os = "linux")]
mod reflink;

pub(crate) trait Strategy {
    fn name(&self) -> &'static str {
        "copy_on_write"
    }

    fn copy_directory(
        &self,
        from: &Path,
        to: &Path,
        mode: CopyMode,
        workspace_id: &str,
    ) -> Result<()>;

    fn initialize_directory(
        &self,
        path: &Path,
        progress: &mut dyn FnMut(InitProgress),
    ) -> Result<StrategyInit>;

    fn remove_directory(&self, path: &Path) -> Result<()> {
        remove_directory_tree(path)
    }
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum StrategyInit {
    AlreadyNative,
    Converted,
}

pub(crate) fn default_strategy() -> Box<dyn Strategy> {
    #[cfg(target_os = "linux")]
    return Box::new(linux::LinuxStrategy);

    #[cfg(target_os = "macos")]
    return Box::new(apfs::ApfsStrategy);

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    return Box::new(UnsupportedStrategy);
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
struct UnsupportedStrategy;

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
impl Strategy for UnsupportedStrategy {
    fn copy_directory(
        &self,
        _from: &Path,
        _to: &Path,
        _mode: CopyMode,
        _workspace_id: &str,
    ) -> Result<()> {
        Err(unsupported_cow_error())
    }

    fn initialize_directory(
        &self,
        _path: &Path,
        _progress: &mut dyn FnMut(InitProgress),
    ) -> Result<StrategyInit> {
        Err(unsupported_cow_error())
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn unsupported_cow_error() -> Error {
    Error::CowUnavailable("no copy-on-write strategy has been implemented for this platform".into())
}

#[cfg(unix)]
fn copy_symlink(from: &Path, to: &Path) -> Result<()> {
    std::os::unix::fs::symlink(fs::read_link(from)?, to)?;
    Ok(())
}

#[cfg(windows)]
fn copy_symlink(from: &Path, to: &Path) -> Result<()> {
    use std::os::windows::fs::FileTypeExt;

    let target = fs::read_link(from)?;
    let file_type = fs::symlink_metadata(from)?.file_type();
    if file_type.is_symlink_dir() {
        std::os::windows::fs::symlink_dir(target, to)?;
    } else if file_type.is_symlink_file() {
        std::os::windows::fs::symlink_file(target, to)?;
    } else {
        return Err(Error::UnsupportedEntry(from.to_path_buf()));
    }
    Ok(())
}

pub(crate) struct PortableCopyStrategy;

impl Strategy for PortableCopyStrategy {
    fn name(&self) -> &'static str {
        "copy"
    }

    fn copy_directory(
        &self,
        from: &Path,
        to: &Path,
        mode: CopyMode,
        workspace_id: &str,
    ) -> Result<()> {
        copy_directory_portable(from, to, mode, workspace_id)
    }

    fn initialize_directory(
        &self,
        _path: &Path,
        _progress: &mut dyn FnMut(InitProgress),
    ) -> Result<StrategyInit> {
        Ok(StrategyInit::AlreadyNative)
    }
}

fn copy_directory_portable(
    from: &Path,
    to: &Path,
    mode: CopyMode,
    workspace_id: &str,
) -> Result<()> {
    create_private_directory(to)?;
    crate::marker::write(to, workspace_id)?;
    let filter = CopyFilter;
    let mut directories = Vec::new();
    let mut hard_links = HashMap::new();
    for entry in walkdir::WalkDir::new(from)
        .min_depth(1)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| {
            !is_workspace_marker(from, entry.path())
                && (mode == CopyMode::All
                    || entry
                        .path()
                        .strip_prefix(from)
                        .map_or(true, |path| !filter.excludes(path)))
        })
    {
        let entry = entry?;
        let destination = to.join(
            entry
                .path()
                .strip_prefix(from)
                .map_err(|error| Error::Path(error.to_string()))?,
        );
        if entry.file_type().is_dir() {
            create_private_directory(&destination)?;
            directories.push((entry.path().to_path_buf(), destination));
            continue;
        }
        if entry.file_type().is_symlink() {
            copy_symlink(entry.path(), &destination)?;
            continue;
        }
        if !entry.file_type().is_file() {
            return Err(Error::UnsupportedEntry(entry.path().to_path_buf()));
        }
        let metadata = fs::symlink_metadata(entry.path())?;
        if let Some(identity) = hard_link_identity(entry.path(), &metadata)? {
            if let Some(existing) = hard_links.get(&identity) {
                fs::hard_link(existing, &destination)?;
            } else {
                fs::copy(entry.path(), &destination)?;
                hard_links.insert(identity, destination);
            }
        } else {
            fs::copy(entry.path(), destination)?;
        }
    }
    for (source, destination) in directories.into_iter().rev() {
        copy_directory_permissions(&source, &destination)?;
    }
    copy_directory_permissions(from, to)
}

#[cfg(unix)]
pub(super) fn create_private_directory(path: &Path) -> Result<()> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};

    let mut builder = fs::DirBuilder::new();
    builder.mode(0o700).create(path)?;
    // A restrictive umask can remove owner bits. Restore only owner access;
    // group and other users must not see a partially populated workspace.
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

#[cfg(not(unix))]
pub(super) fn create_private_directory(path: &Path) -> Result<()> {
    fs::create_dir(path)?;
    Ok(())
}

#[cfg(unix)]
pub(super) fn restrict_directory_to_owner(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

pub(super) fn copy_directory_permissions(from: &Path, to: &Path) -> Result<()> {
    fs::set_permissions(to, fs::symlink_metadata(from)?.permissions())?;
    Ok(())
}

pub(super) fn is_workspace_marker(root: &Path, path: &Path) -> bool {
    path.strip_prefix(root)
        .is_ok_and(|relative| relative == Path::new(crate::marker::FILE_NAME))
}

pub(super) fn ensure_no_mounted_descendants(path: &Path) -> Result<()> {
    mount::ensure_no_mounted_descendants(path)
}

pub(super) fn remove_directory_tree(path: &Path) -> Result<()> {
    ensure_real_directory_for_removal(path)?;
    ensure_no_mounted_descendants(path)?;
    remove_directory_tree_after_mount_check(path)
}

pub(super) fn remove_directory_tree_after_mount_check(path: &Path) -> Result<()> {
    ensure_real_directory_for_removal(path)?;
    make_owned_directories_removable(path)?;

    // Keep the ownership marker until every other entry is gone. If deleting
    // any descendant fails, GC can verify the remaining tree and retry safely.
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        if entry.file_name() == crate::marker::FILE_NAME {
            continue;
        }
        let entry_path = entry.path();
        let file_type = entry.file_type()?;
        #[cfg(windows)]
        {
            use std::os::windows::fs::FileTypeExt;
            if file_type.is_symlink_dir() {
                fs::remove_dir(entry_path)?;
                continue;
            }
        }
        if file_type.is_dir() {
            fs::remove_dir_all(entry_path)?;
        } else {
            fs::remove_file(entry_path)?;
        }
    }

    let marker = crate::marker::read(path)?;
    if marker.is_some() {
        crate::marker::remove(path)?;
    }
    if let Err(error) = fs::remove_dir(path) {
        if let Some(id) = marker {
            crate::marker::write(path, &id)?;
        }
        return Err(error.into());
    }
    Ok(())
}

pub(super) fn ensure_real_directory_for_removal(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(Error::Path(format!(
            "refusing to recursively remove a non-directory path: {}",
            path.display()
        )));
    }
    Ok(())
}

#[cfg(unix)]
pub(super) fn make_owned_directories_removable(path: &Path) -> Result<()> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_dir() {
        return Ok(());
    }

    // SAFETY: geteuid has no preconditions and does not access memory.
    let effective_uid = unsafe { libc::geteuid() };
    if effective_uid != 0 && metadata.uid() != effective_uid {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!(
                "refusing to change permissions on unowned directory {}",
                path.display()
            ),
        )
        .into());
    }

    let mode = metadata.permissions().mode();
    if mode & 0o700 != 0o700 {
        fs::set_permissions(path, fs::Permissions::from_mode(mode | 0o700))?;
    }
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            make_owned_directories_removable(&entry.path())?;
        }
    }
    Ok(())
}

#[cfg(not(unix))]
#[allow(clippy::permissions_set_readonly_false)] // This branch cannot compile on Unix.
pub(super) fn make_owned_directories_removable(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    let file_type = metadata.file_type();
    if file_type.is_symlink() {
        return Ok(());
    }
    let mut permissions = metadata.permissions();
    if permissions.readonly() {
        permissions.set_readonly(false);
        fs::set_permissions(path, permissions)?;
    }
    if file_type.is_dir() {
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            make_owned_directories_removable(&entry.path())?;
        }
    }
    Ok(())
}

#[cfg(unix)]
type FileIdentity = (u64, u64);

#[cfg(unix)]
fn hard_link_identity(_path: &Path, metadata: &fs::Metadata) -> Result<Option<FileIdentity>> {
    use std::os::unix::fs::MetadataExt;

    Ok((metadata.nlink() > 1).then_some((metadata.dev(), metadata.ino())))
}

#[cfg(windows)]
type FileIdentity = (u64, u64);

#[cfg(windows)]
fn hard_link_identity(path: &Path, _metadata: &fs::Metadata) -> Result<Option<FileIdentity>> {
    let handle = winapi_util::Handle::from_path(path)?;
    let information = winapi_util::file::information(&handle)?;
    Ok((information.number_of_links() > 1)
        .then_some((information.volume_serial_number(), information.file_index())))
}

#[cfg(not(any(unix, windows)))]
type FileIdentity = ();

#[cfg(not(any(unix, windows)))]
fn hard_link_identity(_path: &Path, _metadata: &fs::Metadata) -> Result<Option<FileIdentity>> {
    Ok(None)
}

#[cfg(test)]
pub(crate) struct TestStrategy;

#[cfg(test)]
impl Strategy for TestStrategy {
    fn name(&self) -> &'static str {
        "test"
    }

    fn copy_directory(
        &self,
        from: &Path,
        to: &Path,
        mode: CopyMode,
        workspace_id: &str,
    ) -> Result<()> {
        copy_directory_portable(from, to, mode, workspace_id)
    }

    fn initialize_directory(
        &self,
        _path: &Path,
        _progress: &mut dyn FnMut(InitProgress),
    ) -> Result<StrategyInit> {
        Ok(StrategyInit::AlreadyNative)
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use tempfile::TempDir;

    #[test]
    fn portable_copy_preserves_hard_links() {
        use std::os::unix::fs::MetadataExt;

        let temp = TempDir::new().unwrap();
        let source = temp.path().join("source");
        let destination = temp.path().join("destination");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("first.txt"), "shared").unwrap();
        fs::hard_link(source.join("first.txt"), source.join("second.txt")).unwrap();

        copy_directory_portable(
            &source,
            &destination,
            CopyMode::All,
            &ulid::Ulid::new().to_string(),
        )
        .unwrap();

        assert_eq!(
            fs::metadata(destination.join("first.txt")).unwrap().ino(),
            fs::metadata(destination.join("second.txt")).unwrap().ino()
        );
        fs::write(destination.join("first.txt"), "changed").unwrap();
        assert_eq!(
            fs::read_to_string(destination.join("second.txt")).unwrap(),
            "changed"
        );
    }

    #[test]
    fn portable_copy_preserves_directory_permissions() {
        let temp = TempDir::new().unwrap();
        let source = temp.path().join("source");
        let destination = temp.path().join("destination");
        let nested = source.join("nested");
        fs::create_dir(&source).unwrap();
        fs::create_dir(&nested).unwrap();
        fs::write(nested.join("file.txt"), "private").unwrap();
        fs::set_permissions(&source, fs::Permissions::from_mode(0o700)).unwrap();
        fs::set_permissions(&nested, fs::Permissions::from_mode(0o2770)).unwrap();

        copy_directory_portable(
            &source,
            &destination,
            CopyMode::All,
            &ulid::Ulid::new().to_string(),
        )
        .unwrap();

        assert_eq!(
            fs::metadata(&destination).unwrap().permissions().mode() & 0o7777,
            0o700
        );
        assert_eq!(
            fs::metadata(destination.join("nested"))
                .unwrap()
                .permissions()
                .mode()
                & 0o7777,
            0o2770
        );
    }

    #[test]
    fn partial_portable_copies_are_owned_and_private() {
        use std::os::unix::ffi::OsStrExt;

        let temp = TempDir::new().unwrap();
        let source = temp.path().join("source");
        let destination = temp.path().join("destination");
        let nested = source.join("nested");
        let fifo = nested.join("fifo");
        fs::create_dir(&source).unwrap();
        fs::create_dir(&nested).unwrap();
        fs::set_permissions(&source, fs::Permissions::from_mode(0o755)).unwrap();
        fs::set_permissions(&nested, fs::Permissions::from_mode(0o777)).unwrap();
        let fifo_path = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
        // SAFETY: `fifo_path` is a valid C string and the mode is a normal
        // permission bitmask for creating a FIFO in this test.
        assert_eq!(unsafe { libc::mkfifo(fifo_path.as_ptr(), 0o600) }, 0);
        let workspace_id = ulid::Ulid::new().to_string();

        assert!(matches!(
            copy_directory_portable(
                &source,
                &destination,
                CopyMode::All,
                &workspace_id,
            ),
            Err(Error::UnsupportedEntry(path)) if path == fifo
        ));
        crate::marker::verify(&fs::canonicalize(&destination).unwrap(), &workspace_id).unwrap();
        assert_eq!(
            fs::metadata(&destination).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(destination.join("nested"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }

    #[test]
    fn tree_removal_rejects_a_symlinked_root() {
        let temp = TempDir::new().unwrap();
        let external = temp.path().join("external");
        let tree = temp.path().join("tree");
        fs::create_dir(&external).unwrap();
        fs::write(external.join("outside.txt"), "outside").unwrap();
        std::os::unix::fs::symlink(&external, &tree).unwrap();

        assert!(matches!(remove_directory_tree(&tree), Err(Error::Path(_))));
        assert_eq!(
            fs::read_to_string(external.join("outside.txt")).unwrap(),
            "outside"
        );
    }

    #[test]
    fn read_only_tree_removal_does_not_follow_symlinks() {
        let temp = TempDir::new().unwrap();
        let tree = temp.path().join("tree");
        let nested = tree.join("nested");
        let external = temp.path().join("external");
        fs::create_dir(&tree).unwrap();
        fs::create_dir(&nested).unwrap();
        fs::write(nested.join("inside.txt"), "inside").unwrap();
        fs::create_dir(&external).unwrap();
        fs::write(external.join("outside.txt"), "outside").unwrap();
        std::os::unix::fs::symlink(&external, tree.join("external-link")).unwrap();
        fs::set_permissions(&nested, fs::Permissions::from_mode(0o555)).unwrap();
        fs::set_permissions(&tree, fs::Permissions::from_mode(0o555)).unwrap();

        remove_directory_tree(&tree).unwrap();

        assert!(!tree.exists());
        assert_eq!(
            fs::read_to_string(external.join("outside.txt")).unwrap(),
            "outside"
        );
    }
}

#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;
    use std::io::ErrorKind;
    use std::os::windows::fs::{FileTypeExt, symlink_dir, symlink_file};
    use tempfile::TempDir;

    #[test]
    fn portable_copy_preserves_dangling_symlink_kinds() {
        let temp = TempDir::new().unwrap();
        let source = temp.path().join("source");
        let destination = temp.path().join("destination");
        fs::create_dir(&source).unwrap();
        if let Err(error) = symlink_file("missing-file", source.join("file-link")) {
            if error.kind() == ErrorKind::PermissionDenied {
                return;
            }
            panic!("failed to create file symlink: {error}");
        }
        if let Err(error) = symlink_dir("missing-directory", source.join("directory-link")) {
            if error.kind() == ErrorKind::PermissionDenied {
                return;
            }
            panic!("failed to create directory symlink: {error}");
        }

        copy_directory_portable(
            &source,
            &destination,
            CopyMode::All,
            &ulid::Ulid::new().to_string(),
        )
        .unwrap();

        let file_type = fs::symlink_metadata(destination.join("file-link"))
            .unwrap()
            .file_type();
        let directory_type = fs::symlink_metadata(destination.join("directory-link"))
            .unwrap()
            .file_type();
        assert!(file_type.is_symlink_file());
        assert!(directory_type.is_symlink_dir());
        assert_eq!(
            fs::read_link(destination.join("file-link")).unwrap(),
            Path::new("missing-file")
        );
        assert_eq!(
            fs::read_link(destination.join("directory-link")).unwrap(),
            Path::new("missing-directory")
        );
    }

    #[test]
    fn removal_clears_read_only_files() {
        let temp = TempDir::new().unwrap();
        let tree = temp.path().join("tree");
        let nested = tree.join("nested");
        let file = nested.join("read-only.txt");
        fs::create_dir_all(&nested).unwrap();
        fs::write(&file, "contents").unwrap();
        let mut permissions = fs::metadata(&file).unwrap().permissions();
        permissions.set_readonly(true);
        fs::set_permissions(&file, permissions).unwrap();

        remove_directory_tree(&tree).unwrap();

        assert!(!tree.exists());
    }
}
