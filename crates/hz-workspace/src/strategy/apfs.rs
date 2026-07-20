use super::{
    Strategy, StrategyInit, copy_directory_permissions, create_private_directory,
    is_workspace_marker, restrict_directory_to_owner,
};
use crate::{CopyMode, Error, InitProgress, Result, filter::CopyFilter};
use std::fs;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

const MIN_PARALLEL_CLONE_FILES: usize = 32;
const PARALLEL_CLONE_BATCH_SIZE: usize = 1024;
// Higher APFS clone fan-out quickly increases filesystem contention.
const MAX_PARALLEL_CLONE_WORKERS: usize = 4;

pub(super) struct ApfsStrategy;

impl Strategy for ApfsStrategy {
    fn name(&self) -> &'static str {
        "apfs"
    }

    fn copy_directory(
        &self,
        from: &Path,
        to: &Path,
        mode: CopyMode,
        workspace_id: &str,
    ) -> Result<()> {
        match mode {
            CopyMode::All => clone_directory_apfs(from, to, workspace_id),
            CopyMode::Filtered => clone_filtered_directory_apfs(from, to, workspace_id),
        }
    }

    fn initialize_directory(
        &self,
        path: &Path,
        _progress: &mut dyn FnMut(InitProgress),
    ) -> Result<StrategyInit> {
        verify_clonefile_apfs(path)?;
        Ok(StrategyInit::AlreadyNative)
    }
}

fn verify_clonefile_apfs(path: &Path) -> Result<()> {
    let operation_id = ulid::Ulid::new();
    let source = path.join(format!(".hz-clonefile-probe-{operation_id}"));
    let destination = path.join(format!(".hz-clonefile-probe-copy-{operation_id}"));
    fs::write(&source, b"hz")?;
    let result = clone_path_apfs(&source, &destination).map_err(|error| match error {
        Error::CowUnavailable(message) => Error::CowUnavailable(format!(
            "{} does not support macOS copy-on-write clones: {message}",
            path.display()
        )),
        error => error,
    });
    let cleanup = [&source, &destination]
        .into_iter()
        .filter(|candidate| candidate.exists())
        .try_for_each(fs::remove_file);
    result.and(cleanup.map_err(Error::from))
}

fn clone_filtered_directory_apfs(from: &Path, to: &Path, workspace_id: &str) -> Result<()> {
    use std::collections::HashMap;
    use std::os::unix::fs::MetadataExt;

    let filter = CopyFilter;
    let mut hard_links = HashMap::new();
    let mut regular_files = Vec::new();
    let mut directories = Vec::new();
    create_private_directory(to)?;
    crate::marker::write(to, workspace_id)?;
    for entry in WalkDir::new(from)
        .min_depth(1)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| {
            !is_workspace_marker(from, entry.path())
                && entry
                    .path()
                    .strip_prefix(from)
                    .map_or(true, |path| !filter.excludes(path))
        })
    {
        let entry = entry?;
        let source = entry.path();
        let destination = to.join(
            source
                .strip_prefix(from)
                .map_err(|error| Error::Path(error.to_string()))?,
        );
        let metadata = fs::symlink_metadata(source)?;
        let file_type = metadata.file_type();
        if file_type.is_dir() {
            create_private_directory(&destination)?;
            directories.push((source.to_path_buf(), destination));
        } else if file_type.is_file() {
            let key = (metadata.dev(), metadata.ino());
            if metadata.nlink() > 1 {
                if let Some(existing) = hard_links.get(&key) {
                    fs::hard_link(existing, &destination)?;
                } else {
                    clone_regular_file_apfs(source, &destination, metadata.mode())?;
                    hard_links.insert(key, destination.clone());
                }
            } else {
                // clonefile with CLONE_ACL already preserves regular-file
                // metadata and xattrs except for set-ID mode bits.
                regular_files.push((source.to_path_buf(), destination, metadata.mode()));
                if regular_files.len() == PARALLEL_CLONE_BATCH_SIZE {
                    clone_regular_files_apfs(&regular_files)?;
                    regular_files.clear();
                }
            }
        } else if file_type.is_symlink() {
            std::os::unix::fs::symlink(fs::read_link(source)?, &destination)?;
            copy_metadata_apfs(source, &destination, MetadataTarget::Symlink)?;
        } else {
            return Err(Error::UnsupportedEntry(source.to_path_buf()));
        }
    }
    clone_regular_files_apfs(&regular_files)?;
    for (source, destination) in directories.into_iter().rev() {
        copy_metadata_apfs(&source, &destination, MetadataTarget::FileOrDirectory)?;
    }
    copy_metadata_apfs(from, to, MetadataTarget::FileOrDirectory)?;
    Ok(())
}

fn clone_regular_files_apfs(files: &[(PathBuf, PathBuf, u32)]) -> Result<()> {
    let worker_count = if files.len() < MIN_PARALLEL_CLONE_FILES {
        1
    } else {
        std::thread::available_parallelism()
            .map_or(1, usize::from)
            .min(MAX_PARALLEL_CLONE_WORKERS)
            .min(files.len())
    };
    if worker_count <= 1 {
        return files
            .iter()
            .try_for_each(|(from, to, mode)| clone_regular_file_apfs(from, to, *mode));
    }

    std::thread::scope(|scope| {
        let workers = (0..worker_count)
            .map(|offset| {
                scope.spawn(move || {
                    files
                        .iter()
                        .skip(offset)
                        .step_by(worker_count)
                        .try_for_each(|(from, to, mode)| clone_regular_file_apfs(from, to, *mode))
                })
            })
            .collect::<Vec<_>>();
        for worker in workers {
            match worker.join() {
                Ok(result) => result?,
                Err(panic) => std::panic::resume_unwind(panic),
            }
        }
        Ok(())
    })
}

fn clone_regular_file_apfs(from: &Path, to: &Path, source_mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    clone_path_apfs(from, to)?;
    // clonefile always clears these bits on regular files. Avoid an extra
    // chmod for the overwhelmingly common case where neither bit was set.
    if source_mode & 0o6000 != 0 {
        fs::set_permissions(to, fs::Permissions::from_mode(source_mode))?;
    }
    Ok(())
}

fn clone_directory_apfs(from: &Path, to: &Path, workspace_id: &str) -> Result<()> {
    clone_path_apfs(from, to)?;
    restrict_directory_to_owner(to)?;
    crate::marker::write(to, workspace_id)?;
    copy_directory_permissions(from, to)
}

fn clone_path_apfs(from: &Path, to: &Path) -> Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    // CLONE_ACL from <sys/clonefile.h>. Without it, clonefile replaces source
    // ACLs with ACLs inherited from the destination parent.
    const CLONE_ACL: u32 = 0x0004;

    let source = CString::new(from.as_os_str().as_bytes())
        .map_err(|_| Error::Path(format!("path contains a null byte: {}", from.display())))?;
    let destination = CString::new(to.as_os_str().as_bytes())
        .map_err(|_| Error::Path(format!("path contains a null byte: {}", to.display())))?;
    // SAFETY: `source` and `destination` are null-terminated C strings
    // built above, and both live for the duration of the call.
    let result = unsafe { libc::clonefile(source.as_ptr(), destination.as_ptr(), CLONE_ACL) };
    if result == 0 {
        return Ok(());
    }
    Err(Error::CowUnavailable(format!(
        "failed to clone {}: {}",
        from.display(),
        std::io::Error::last_os_error()
    )))
}

#[derive(Clone, Copy)]
enum MetadataTarget {
    FileOrDirectory,
    Symlink,
}

fn copy_metadata_apfs(from: &Path, to: &Path, target: MetadataTarget) -> Result<()> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let metadata = fs::symlink_metadata(from)?;
    let destination = c_path(to)?;
    // SAFETY: `destination` is a valid null-terminated path, and uid/gid come
    // from filesystem metadata for `from`.
    if unsafe { libc::lchown(destination.as_ptr(), metadata.uid(), metadata.gid()) } != 0 {
        let error = std::io::Error::last_os_error();
        // A caller that can read an entry may still be unable to assume its
        // container-created owner or group. Keep caller ownership in that case.
        if error.kind() != std::io::ErrorKind::PermissionDenied {
            return Err(error.into());
        }
    }
    if matches!(target, MetadataTarget::FileOrDirectory) {
        fs::set_permissions(to, fs::Permissions::from_mode(metadata.mode()))?;
    }
    copy_xattrs_apfs(from, to)?;
    let times = [
        libc::timespec {
            tv_sec: metadata.atime(),
            tv_nsec: metadata.atime_nsec(),
        },
        libc::timespec {
            tv_sec: metadata.mtime(),
            tv_nsec: metadata.mtime_nsec(),
        },
    ];
    // SAFETY: `destination` is a live C string and `times` contains exactly the
    // two timestamps expected by `utimensat`.
    if unsafe {
        libc::utimensat(
            libc::AT_FDCWD,
            destination.as_ptr(),
            times.as_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    } != 0
    {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}

fn copy_xattrs_apfs(from: &Path, to: &Path) -> Result<()> {
    let from = c_path(from)?;
    let to = c_path(to)?;
    // SAFETY: `from` is a valid C path. A null buffer with size 0 asks the
    // kernel for the required list size.
    let size =
        unsafe { libc::listxattr(from.as_ptr(), std::ptr::null_mut(), 0, libc::XATTR_NOFOLLOW) };
    if size < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let mut names = vec![0_u8; size as usize];
    // SAFETY: `names` was allocated with the size reported by the previous
    // `listxattr` call, and its pointer is valid for writes of that length.
    if size > 0
        && unsafe {
            libc::listxattr(
                from.as_ptr(),
                names.as_mut_ptr().cast(),
                names.len(),
                libc::XATTR_NOFOLLOW,
            )
        } < 0
    {
        return Err(std::io::Error::last_os_error().into());
    }
    for name in names
        .split(|byte| *byte == 0)
        .filter(|name| !name.is_empty())
    {
        // macOS manages this attribute and may reject attempts to set it even
        // when clonefile already preserved it on the destination.
        if name == b"com.apple.provenance" {
            continue;
        }
        let name = std::ffi::CString::new(name)
            .map_err(|_| Error::Path("extended attribute name contains a null byte".into()))?;
        // SAFETY: `from` and `name` are valid C strings. A null buffer with
        // size 0 asks the kernel for this attribute's value length.
        let size = unsafe {
            libc::getxattr(
                from.as_ptr(),
                name.as_ptr(),
                std::ptr::null_mut(),
                0,
                0,
                libc::XATTR_NOFOLLOW,
            )
        };
        if size < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        let mut value = vec![0_u8; size as usize];
        // SAFETY: `value` was allocated with the exact size reported by
        // `getxattr`, and the path and attribute name are valid C strings.
        if size > 0
            && unsafe {
                libc::getxattr(
                    from.as_ptr(),
                    name.as_ptr(),
                    value.as_mut_ptr().cast(),
                    value.len(),
                    0,
                    libc::XATTR_NOFOLLOW,
                )
            } < 0
        {
            return Err(std::io::Error::last_os_error().into());
        }
        // SAFETY: `to`, `name`, and `value` are valid for the duration of the
        // call. `XATTR_NOFOLLOW` keeps symlink behavior consistent.
        if unsafe {
            libc::setxattr(
                to.as_ptr(),
                name.as_ptr(),
                value.as_ptr().cast(),
                value.len(),
                0,
                libc::XATTR_NOFOLLOW,
            )
        } != 0
        {
            let error = std::io::Error::last_os_error();
            if error.kind() != std::io::ErrorKind::PermissionDenied {
                return Err(error.into());
            }
        }
    }
    Ok(())
}

fn c_path(path: &Path) -> Result<std::ffi::CString> {
    use std::os::unix::ffi::OsStrExt;

    std::ffi::CString::new(path.as_os_str().as_bytes())
        .map_err(|_| Error::Path(format!("path contains a null byte: {}", path.display())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    use tempfile::TempDir;

    fn set_test_xattr(path: &Path, value: &[u8]) {
        let path = c_path(path).unwrap();
        let name = c"com.hz.filtered-test";
        assert_eq!(
            // SAFETY: test inputs are live C strings and `value` is copied by the kernel.
            unsafe {
                libc::setxattr(
                    path.as_ptr(),
                    name.as_ptr(),
                    value.as_ptr().cast(),
                    value.len(),
                    0,
                    libc::XATTR_NOFOLLOW,
                )
            },
            0
        );
    }

    fn test_xattr(path: &Path) -> Vec<u8> {
        let path = c_path(path).unwrap();
        let name = c"com.hz.filtered-test";
        // SAFETY: test inputs are valid C strings and a null buffer requests the value size.
        let size = unsafe {
            libc::getxattr(
                path.as_ptr(),
                name.as_ptr(),
                std::ptr::null_mut(),
                0,
                0,
                libc::XATTR_NOFOLLOW,
            )
        };
        assert!(size >= 0);
        let mut value = vec![0; size as usize];
        // SAFETY: `value` has the exact size returned by the preceding query.
        assert_eq!(
            unsafe {
                libc::getxattr(
                    path.as_ptr(),
                    name.as_ptr(),
                    value.as_mut_ptr().cast(),
                    value.len(),
                    0,
                    libc::XATTR_NOFOLLOW,
                )
            },
            size
        );
        value
    }

    #[test]
    fn strategy_clones_and_removes_a_workspace() {
        let temp = TempDir::new().unwrap();
        let source = temp.path().join("source");
        let destination = temp.path().join("destination");
        fs::create_dir(&source).unwrap();
        fs::create_dir(source.join("nested")).unwrap();
        fs::write(source.join("nested/file.txt"), "hello").unwrap();
        let strategy = ApfsStrategy;

        assert_eq!(
            strategy
                .initialize_directory(temp.path(), &mut |_| {})
                .unwrap(),
            StrategyInit::AlreadyNative
        );
        strategy
            .copy_directory(
                &source,
                &destination,
                CopyMode::All,
                &ulid::Ulid::new().to_string(),
            )
            .unwrap();
        assert_eq!(
            fs::read_to_string(destination.join("nested/file.txt")).unwrap(),
            "hello"
        );
        strategy.remove_directory(&destination).unwrap();
        assert!(!destination.exists());
    }

    #[test]
    fn clone_path_preserves_source_acls() {
        use std::process::Command;

        let temp = TempDir::new().unwrap();
        let source = temp.path().join("source");
        let destination = temp.path().join("destination");
        fs::write(&source, "source").unwrap();
        let chmod = Command::new("chmod")
            .arg("+a")
            .arg("everyone allow read")
            .arg(&source)
            .output()
            .unwrap();
        assert!(
            chmod.status.success(),
            "failed to set test ACL: {}",
            String::from_utf8_lossy(&chmod.stderr)
        );

        clone_path_apfs(&source, &destination).unwrap();

        let listed = Command::new("ls")
            .arg("-lde")
            .arg(&destination)
            .output()
            .unwrap();
        assert!(listed.status.success());
        assert!(
            String::from_utf8_lossy(&listed.stdout).contains("everyone allow read"),
            "destination ACL was not copied: {}",
            String::from_utf8_lossy(&listed.stdout)
        );
    }

    #[test]
    fn filtered_clone_skips_git_fsmonitor_sockets() {
        use std::os::unix::net::UnixListener;

        let temp = TempDir::new().unwrap();
        let source = temp.path().join("source");
        let destination = temp.path().join("destination");
        fs::create_dir_all(source.join(".git")).unwrap();
        fs::write(source.join("tracked"), "tracked").unwrap();
        let socket = source.join(".git/fsmonitor--daemon.ipc");
        drop(UnixListener::bind(&socket).unwrap());

        clone_filtered_directory_apfs(&source, &destination, &ulid::Ulid::new().to_string())
            .unwrap();

        assert_eq!(
            fs::read_to_string(destination.join("tracked")).unwrap(),
            "tracked"
        );
        assert!(!destination.join(".git/fsmonitor--daemon.ipc").exists());
    }

    #[test]
    fn filtered_clone_preserves_set_id_mode_bits() {
        let temp = TempDir::new().unwrap();
        let source = temp.path().join("source");
        let destination = temp.path().join("destination");
        fs::create_dir(&source).unwrap();
        let file = source.join("set-id");
        fs::write(&file, "executable").unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o6751)).unwrap();

        clone_filtered_directory_apfs(&source, &destination, &ulid::Ulid::new().to_string())
            .unwrap();

        assert_eq!(
            fs::metadata(destination.join("set-id"))
                .unwrap()
                .permissions()
                .mode()
                & 0o7777,
            0o6751
        );
    }

    #[test]
    fn regular_file_batches_clone_all_entries() {
        let temp = TempDir::new().unwrap();
        let source = temp.path().join("source");
        let destination = temp.path().join("destination");
        fs::create_dir(&source).unwrap();
        fs::create_dir(&destination).unwrap();
        let files = (0..64)
            .map(|index| {
                let from = source.join(format!("file-{index}"));
                let to = destination.join(format!("file-{index}"));
                fs::write(&from, index.to_string()).unwrap();
                (from, to, 0o644)
            })
            .collect::<Vec<_>>();

        clone_regular_files_apfs(&files).unwrap();

        for (from, to, _) in files {
            assert_eq!(fs::read(from).unwrap(), fs::read(to).unwrap());
        }
    }

    #[test]
    fn integration_environment_is_required_by_ci() {
        if std::env::var_os("RIFT_REQUIRE_APFS_TESTS").is_some() {
            let temp = TempDir::new().unwrap();
            let source = temp.path().join("source");
            let destination = temp.path().join("destination");
            fs::create_dir(&source).unwrap();
            assert!(
                ApfsStrategy
                    .copy_directory(
                        &source,
                        &destination,
                        CopyMode::All,
                        &ulid::Ulid::new().to_string(),
                    )
                    .is_ok()
            );
        }
    }

    #[test]
    fn filtered_strategy_preserves_included_metadata_and_hard_links() {
        let temp = TempDir::new().unwrap();
        let source = temp.path().join("source");
        let destination = temp.path().join("destination");
        let nested = source.join("nested");
        fs::create_dir(&source).unwrap();
        fs::set_permissions(&source, fs::Permissions::from_mode(0o750)).unwrap();
        fs::create_dir(&nested).unwrap();
        fs::set_permissions(&nested, fs::Permissions::from_mode(0o710)).unwrap();
        let file = nested.join("file.txt");
        fs::write(&file, "hello").unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o640)).unwrap();
        set_test_xattr(&file, b"preserved");
        fs::hard_link(&file, nested.join("hard.txt")).unwrap();
        std::os::unix::fs::symlink("file.txt", nested.join("link.txt")).unwrap();
        fs::create_dir_all(source.join("node_modules/pkg")).unwrap();
        fs::write(source.join("node_modules/pkg/index.js"), "module").unwrap();

        ApfsStrategy
            .copy_directory(
                &source,
                &destination,
                CopyMode::Filtered,
                &ulid::Ulid::new().to_string(),
            )
            .unwrap();

        assert!(!destination.join("node_modules").exists());
        assert_eq!(
            fs::read_to_string(destination.join("nested/file.txt")).unwrap(),
            "hello"
        );
        assert_eq!(
            fs::read_link(destination.join("nested/link.txt")).unwrap(),
            Path::new("file.txt")
        );
        assert_eq!(
            fs::metadata(destination.join("nested/file.txt"))
                .unwrap()
                .ino(),
            fs::metadata(destination.join("nested/hard.txt"))
                .unwrap()
                .ino()
        );
        assert_eq!(
            fs::metadata(destination.join("nested/file.txt"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o640
        );
        assert_eq!(
            test_xattr(&destination.join("nested/file.txt")),
            b"preserved"
        );
        assert_eq!(
            fs::metadata(destination.join("nested"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o710
        );
        assert_eq!(
            fs::metadata(&destination).unwrap().permissions().mode() & 0o777,
            0o750
        );
    }
}
