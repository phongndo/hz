use super::linux::{Filesystem, filesystem};
use super::mount::ensure_no_mounted_descendants;
use super::reflink::{
    LinuxReflinkStrategy, MetadataTarget, copy_metadata_linux, import_directory_linux,
    import_directory_linux_filtered,
};
use super::{
    Strategy, StrategyInit, copy_directory_permissions, ensure_real_directory_for_removal,
    remove_directory_tree_after_mount_check, restrict_directory_to_owner,
};
use crate::{CopyMode, Error, InitProgress, Result};
use std::fs;
use std::path::Path;

pub(super) struct BtrfsStrategy;

impl Strategy for BtrfsStrategy {
    fn copy_directory(
        &self,
        from: &Path,
        to: &Path,
        mode: CopyMode,
        workspace_id: &str,
    ) -> Result<()> {
        copy_directory_linux(from, to, mode, workspace_id)
    }

    fn initialize_directory(
        &self,
        path: &Path,
        progress: &mut dyn FnMut(InitProgress),
    ) -> Result<StrategyInit> {
        initialize_directory_linux(path, progress)
    }

    fn remove_directory(&self, path: &Path) -> Result<()> {
        remove_directory_linux(path)
    }
}

fn copy_directory_linux(from: &Path, to: &Path, mode: CopyMode, workspace_id: &str) -> Result<()> {
    if !is_btrfs_filesystem(from)? {
        return Err(Error::CowUnavailable(format!(
            "Linux snapshot creation requires btrfs; {} is on another filesystem",
            from.display()
        )));
    }
    let destination_parent = to
        .parent()
        .ok_or_else(|| Error::Path(format!("destination has no parent: {}", to.display())))?;
    if !btrfs_subvolume_deletion_available(destination_parent)? {
        return LinuxReflinkStrategy.copy_directory(from, to, mode, workspace_id);
    }
    match mode {
        CopyMode::All => {
            if is_btrfs_subvolume(from)? && !contains_nested_btrfs_subvolume(from)? {
                create_owned_btrfs_snapshot(from, to, workspace_id)
            } else {
                create_imported_btrfs_subvolume(from, to, false, workspace_id)
            }
        }
        CopyMode::Filtered => create_imported_btrfs_subvolume(from, to, true, workspace_id),
    }
}

#[cfg(target_os = "linux")]
fn contains_nested_btrfs_subvolume(path: &Path) -> Result<bool> {
    use std::fs::File;
    use std::os::fd::AsRawFd;

    let directory = File::open(path)?;
    let mut rootrefs = BtrfsIoctlGetSubvolRootrefArgs {
        min_treeid: 0,
        rootref: [BtrfsIoctlRootref {
            treeid: 0,
            dirid: 0,
        }; BTRFS_MAX_ROOTREF_BUFFER_NUM],
        num_items: 0,
        align: [0; 7],
    };
    // SAFETY: `directory` is an open btrfs directory and `rootrefs` has the C
    // layout and 4096-byte size required by BTRFS_IOC_GET_SUBVOL_ROOTREF.
    let result = unsafe {
        libc::ioctl(
            directory.as_raw_fd(),
            BTRFS_IOC_GET_SUBVOL_ROOTREF,
            &mut rootrefs,
        )
    };
    if result == 0 {
        return Ok(rootrefs.num_items != 0);
    }
    let error = std::io::Error::last_os_error();
    // More than 255 direct children is still a definitive positive result;
    // the kernel fills this first page before reporting EOVERFLOW.
    if error.raw_os_error() == Some(libc::EOVERFLOW) {
        return Ok(rootrefs.num_items != 0);
    }
    Err(error.into())
}

#[cfg(target_os = "linux")]
fn create_imported_btrfs_subvolume(
    from: &Path,
    to: &Path,
    filtered: bool,
    workspace_id: &str,
) -> Result<()> {
    create_btrfs_subvolume(to)?;
    restrict_directory_to_owner(to)?;
    crate::marker::write(to, workspace_id)?;
    let result = if filtered {
        import_directory_linux_filtered(from, to, &mut |_| {})
    } else {
        import_directory_linux(from, to, &mut |_| {})
    }
    .and_then(|()| copy_metadata_linux(from, to, MetadataTarget::FileOrDirectory));
    if let Err(error) = result {
        let _ = remove_directory_linux(to);
        return Err(error);
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn initialize_directory_linux(
    path: &Path,
    progress: &mut dyn FnMut(InitProgress),
) -> Result<StrategyInit> {
    if !is_btrfs_filesystem(path)? {
        return Err(Error::CowUnavailable(format!(
            "{} is not on a btrfs filesystem",
            path.display()
        )));
    }
    if !btrfs_subvolume_deletion_available(path)? {
        return LinuxReflinkStrategy.initialize_directory(path, progress);
    }
    if !is_btrfs_subvolume(path)? {
        // An ordinary directory cannot be snapshotted atomically. Replacing it
        // with an imported subvolume could discard concurrent writes, so keep
        // the live root and import child workspaces into subvolumes instead.
        ensure_no_mounted_descendants(path)?;
    }
    Ok(StrategyInit::AlreadyNative)
}

#[cfg(target_os = "linux")]
fn remove_directory_linux(path: &Path) -> Result<()> {
    ensure_real_directory_for_removal(path)?;
    ensure_no_mounted_descendants(path)?;
    if !is_btrfs_subvolume(path)? {
        return remove_directory_tree_after_mount_check(path);
    }
    delete_btrfs_subvolume(path)
}

#[cfg(target_os = "linux")]
fn is_btrfs_subvolume(path: &Path) -> Result<bool> {
    use std::os::unix::fs::MetadataExt;

    if !is_btrfs_filesystem(path)? {
        return Ok(false);
    }
    Ok(fs::metadata(path)?.ino() == 256)
}

#[cfg(target_os = "linux")]
fn is_btrfs_filesystem(path: &Path) -> Result<bool> {
    Ok(matches!(filesystem(path)?, Filesystem::Btrfs))
}

#[cfg(target_os = "linux")]
fn create_btrfs_subvolume(path: &Path) -> Result<()> {
    btrfs_path_ioctl(path, BTRFS_IOC_SUBVOL_CREATE, None, "create subvolume")
}

#[cfg(target_os = "linux")]
fn btrfs_subvolume_deletion_available(parent: &Path) -> Result<bool> {
    let probe = parent.join(format!(".hz-btrfs-delete-probe-{}", ulid::Ulid::new()));
    match btrfs_path_ioctl(
        &probe,
        BTRFS_IOC_SNAP_DESTROY,
        None,
        "probe subvolume deletion",
    ) {
        Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => Ok(true),
        Err(Error::Io(error))
            if matches!(
                error.kind(),
                std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::Unsupported
            ) =>
        {
            Ok(false)
        }
        Ok(()) => Err(Error::RegistryInvariant(format!(
            "nonexistent btrfs deletion probe unexpectedly succeeded: {}",
            probe.display()
        ))),
        Err(error) => Err(error),
    }
}

#[cfg(target_os = "linux")]
fn create_btrfs_snapshot(from: &Path, to: &Path) -> Result<()> {
    use std::fs::File;
    use std::os::fd::AsRawFd;

    let source = File::open(from)?;
    btrfs_path_ioctl(
        to,
        BTRFS_IOC_SNAP_CREATE,
        Some(source.as_raw_fd()),
        "snapshot",
    )
}

#[cfg(target_os = "linux")]
fn create_owned_btrfs_snapshot(from: &Path, to: &Path, workspace_id: &str) -> Result<()> {
    create_btrfs_snapshot(from, to)?;
    restrict_directory_to_owner(to)?;
    crate::marker::write(to, workspace_id)?;
    copy_directory_permissions(from, to)
}

#[cfg(target_os = "linux")]
fn delete_btrfs_subvolume(path: &Path) -> Result<()> {
    // Never empty a subvolume after a permission failure: rmdir cannot remove
    // a subvolume either, and doing so would destroy its contents while
    // leaving GC unable to finish. The marker and contents must remain intact
    // so deletion can be retried after mount permissions are corrected.
    btrfs_path_ioctl(path, BTRFS_IOC_SNAP_DESTROY, None, "delete subvolume")
}

#[cfg(target_os = "linux")]
const BTRFS_MAX_ROOTREF_BUFFER_NUM: usize = 255;
#[cfg(target_os = "linux")]
const BTRFS_IOC_GET_SUBVOL_ROOTREF: libc::c_ulong = 0xd000_943d;
#[cfg(target_os = "linux")]
const BTRFS_IOC_SNAP_CREATE: libc::c_ulong = 0x5000_9401;
#[cfg(target_os = "linux")]
const BTRFS_IOC_SUBVOL_CREATE: libc::c_ulong = 0x5000_940e;
#[cfg(target_os = "linux")]
const BTRFS_IOC_SNAP_DESTROY: libc::c_ulong = 0x5000_940f;

#[cfg(target_os = "linux")]
#[repr(C)]
#[derive(Clone, Copy)]
struct BtrfsIoctlRootref {
    treeid: u64,
    dirid: u64,
}

#[cfg(target_os = "linux")]
#[repr(C)]
struct BtrfsIoctlGetSubvolRootrefArgs {
    min_treeid: u64,
    rootref: [BtrfsIoctlRootref; BTRFS_MAX_ROOTREF_BUFFER_NUM],
    num_items: u8,
    align: [u8; 7],
}

#[cfg(target_os = "linux")]
const _: () = assert!(std::mem::size_of::<BtrfsIoctlGetSubvolRootrefArgs>() == 4096);

#[cfg(target_os = "linux")]
#[repr(C)]
struct BtrfsIoctlVolArgs {
    fd: i64,
    name: [libc::c_char; 4088],
}

#[cfg(target_os = "linux")]
fn btrfs_path_ioctl(
    path: &Path,
    request: libc::c_ulong,
    source_fd: Option<libc::c_int>,
    action: &str,
) -> Result<()> {
    use std::fs::File;
    use std::os::fd::AsRawFd;
    use std::os::unix::ffi::OsStrExt;

    let parent = path
        .parent()
        .ok_or_else(|| Error::Path(format!("path has no parent: {}", path.display())))?;
    let name = path
        .file_name()
        .ok_or_else(|| Error::Path(format!("path has no name: {}", path.display())))?
        .as_bytes();
    if name.is_empty() || name.len() >= 4088 || name.contains(&0) {
        return Err(Error::Path(format!(
            "invalid btrfs subvolume name: {}",
            path.display()
        )));
    }
    let mut args = BtrfsIoctlVolArgs {
        fd: source_fd.map_or(0, i64::from),
        name: [0; 4088],
    };
    for (destination, byte) in args.name.iter_mut().zip(name) {
        *destination = *byte as libc::c_char;
    }
    let parent = File::open(parent)?;
    // SAFETY: `parent` is an open directory fd, and `args` has the C layout
    // expected by the btrfs volume ioctls for the duration of this call.
    let result = unsafe { libc::ioctl(parent.as_raw_fd(), request, &args) };
    if result == 0 {
        return Ok(());
    }
    let error = std::io::Error::last_os_error();
    if request == BTRFS_IOC_SNAP_DESTROY {
        return Err(Error::Io(error));
    }
    Err(Error::CowUnavailable(format!(
        "failed to {action} {}: {error}",
        path.display()
    )))
}

#[cfg(all(test, target_os = "linux"))]
mod linux_tests {
    use super::*;
    use crate::strategy::reflink::c_path;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    use tempfile::{Builder, TempDir};

    fn btrfs_temp() -> Option<TempDir> {
        let temp = Builder::new()
            .prefix(".rift-core-test-")
            .tempdir_in(std::env::current_dir().unwrap())
            .unwrap();
        is_btrfs_filesystem(temp.path()).unwrap().then_some(temp)
    }

    #[test]
    fn btrfs_integration_environment_is_available() {
        if std::env::var_os("RIFT_REQUIRE_BTRFS_TESTS").is_some() {
            assert!(
                btrfs_temp().is_some(),
                "RIFT_REQUIRE_BTRFS_TESTS requires the checkout filesystem to be btrfs"
            );
        }
    }

    fn set_xattr(path: &Path, name: &str, value: &[u8]) {
        let path = c_path(path).unwrap();
        let name = std::ffi::CString::new(name).unwrap();
        assert_eq!(
            // SAFETY: test inputs are valid C strings and `value` is a live
            // byte slice whose contents are copied by the kernel.
            unsafe {
                libc::lsetxattr(
                    path.as_ptr(),
                    name.as_ptr(),
                    value.as_ptr().cast(),
                    value.len(),
                    0,
                )
            },
            0
        );
    }

    fn get_xattr(path: &Path, name: &str) -> Vec<u8> {
        let path = c_path(path).unwrap();
        let name = std::ffi::CString::new(name).unwrap();
        // SAFETY: test inputs are valid C strings. A null buffer with size 0
        // requests the attribute value length.
        let size =
            unsafe { libc::lgetxattr(path.as_ptr(), name.as_ptr(), std::ptr::null_mut(), 0) };
        assert!(size >= 0);
        let mut value = vec![0; size as usize];
        assert_eq!(
            // SAFETY: `value` is allocated with the exact size returned by
            // `lgetxattr`, and the C strings live for this call.
            unsafe {
                libc::lgetxattr(
                    path.as_ptr(),
                    name.as_ptr(),
                    value.as_mut_ptr().cast(),
                    value.len(),
                )
            },
            size
        );
        value
    }

    #[test]
    fn native_init_preserves_an_ordinary_root_and_imports_its_child() {
        let Some(temp) = btrfs_temp() else {
            return;
        };
        let uses_subvolumes = btrfs_subvolume_deletion_available(temp.path()).unwrap();
        let source = temp.path().join("source");
        let child = temp.path().join("child");
        fs::create_dir(&source).unwrap();
        fs::set_permissions(&source, fs::Permissions::from_mode(0o750)).unwrap();
        let nested = source.join("nested");
        fs::create_dir(&nested).unwrap();
        fs::set_permissions(&nested, fs::Permissions::from_mode(0o700)).unwrap();
        let file = nested.join("file.txt");
        fs::write(&file, "hello").unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o640)).unwrap();
        set_xattr(&file, "user.rift_test", b"xattr");
        fs::hard_link(&file, nested.join("hard.txt")).unwrap();
        std::os::unix::fs::symlink("file.txt", nested.join("link.txt")).unwrap();

        assert_eq!(
            initialize_directory_linux(&source, &mut |_| {}).unwrap(),
            StrategyInit::AlreadyNative
        );
        assert!(!is_btrfs_subvolume(&source).unwrap());
        assert_eq!(fs::read_to_string(&file).unwrap(), "hello");

        copy_directory_linux(
            &source,
            &child,
            CopyMode::All,
            &ulid::Ulid::new().to_string(),
        )
        .unwrap();
        assert_eq!(is_btrfs_subvolume(&child).unwrap(), uses_subvolumes);
        assert_eq!(
            fs::read_to_string(child.join("nested/file.txt")).unwrap(),
            "hello"
        );
        assert_eq!(
            fs::read_link(child.join("nested/link.txt")).unwrap(),
            Path::new("file.txt")
        );
        assert_eq!(
            fs::metadata(child.join("nested/file.txt")).unwrap().ino(),
            fs::metadata(child.join("nested/hard.txt")).unwrap().ino()
        );
        assert_eq!(
            fs::metadata(child.join("nested/file.txt"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o640
        );
        assert_eq!(
            get_xattr(&child.join("nested/file.txt"), "user.rift_test"),
            b"xattr"
        );
        remove_directory_linux(&child).unwrap();
    }

    #[test]
    fn snapshots_import_nested_subvolume_contents() {
        let Some(temp) = btrfs_temp() else {
            return;
        };
        if !btrfs_subvolume_deletion_available(temp.path()).unwrap() {
            return;
        }
        let source = temp.path().join("source");
        let nested = source.join("nested");
        let snapshot = temp.path().join("snapshot");
        create_btrfs_subvolume(&source).unwrap();
        create_btrfs_subvolume(&nested).unwrap();
        fs::write(nested.join("file.txt"), "nested contents").unwrap();

        assert!(contains_nested_btrfs_subvolume(&source).unwrap());
        copy_directory_linux(
            &source,
            &snapshot,
            CopyMode::All,
            &ulid::Ulid::new().to_string(),
        )
        .unwrap();

        assert_eq!(
            fs::read_to_string(snapshot.join("nested/file.txt")).unwrap(),
            "nested contents"
        );
        assert!(!is_btrfs_subvolume(&snapshot.join("nested")).unwrap());

        remove_directory_linux(&snapshot).unwrap();
        remove_directory_linux(&nested).unwrap();
        remove_directory_linux(&source).unwrap();
    }

    #[test]
    fn native_snapshot_and_delete_use_btrfs_strategy() {
        let Some(temp) = btrfs_temp() else {
            return;
        };
        if !btrfs_subvolume_deletion_available(temp.path()).unwrap() {
            return;
        }
        let source = temp.path().join("source");
        let snapshot = temp.path().join("snapshot");
        let filtered = temp.path().join("filtered");
        create_btrfs_subvolume(&source).unwrap();
        fs::write(source.join("file.txt"), "shared before mutation").unwrap();

        copy_directory_linux(
            &source,
            &snapshot,
            CopyMode::All,
            &ulid::Ulid::new().to_string(),
        )
        .unwrap();
        assert!(is_btrfs_subvolume(&snapshot).unwrap());
        assert_eq!(
            fs::read_to_string(snapshot.join("file.txt")).unwrap(),
            "shared before mutation"
        );
        assert_copy_diverges_after_mutation(&source.join("file.txt"), &snapshot.join("file.txt"));

        copy_directory_linux(
            &source,
            &filtered,
            CopyMode::Filtered,
            &ulid::Ulid::new().to_string(),
        )
        .unwrap();
        assert!(is_btrfs_subvolume(&filtered).unwrap());
        assert_copy_diverges_after_mutation(&source.join("file.txt"), &filtered.join("file.txt"));

        remove_directory_linux(&filtered).unwrap();
        remove_directory_linux(&snapshot).unwrap();
        remove_directory_linux(&source).unwrap();
        assert!(!filtered.exists());
        assert!(!snapshot.exists());
        assert!(!source.exists());
    }

    #[test]
    fn native_strategy_reports_non_btrfs_and_unsupported_entries() {
        let temp = TempDir::new().unwrap();
        if !is_btrfs_filesystem(temp.path()).unwrap() {
            assert!(!is_btrfs_subvolume(temp.path()).unwrap());
            assert!(matches!(
                initialize_directory_linux(temp.path(), &mut |_| {}),
                Err(Error::CowUnavailable(_))
            ));
            assert!(matches!(
                copy_directory_linux(
                    temp.path(),
                    &temp.path().join("snapshot"),
                    CopyMode::All,
                    &ulid::Ulid::new().to_string(),
                ),
                Err(Error::CowUnavailable(_))
            ));
        }

        let Some(btrfs) = btrfs_temp() else {
            return;
        };
        let from = btrfs.path().join("source");
        let to = btrfs.path().join("destination");
        fs::create_dir(&from).unwrap();
        fs::create_dir(&to).unwrap();
        let fifo = from.join("fifo");
        let fifo_name = c_path(&fifo).unwrap();
        // SAFETY: `fifo_name` is a valid C path and the mode is a normal
        // permission bitmask for creating a FIFO in this test.
        assert_eq!(unsafe { libc::mkfifo(fifo_name.as_ptr(), 0o600) }, 0);
        assert!(matches!(
            import_directory_linux(&from, &to, &mut |_| {}),
            Err(Error::UnsupportedEntry(path)) if path == fifo
        ));
    }

    #[test]
    fn native_removal_removes_a_populated_ordinary_directory() {
        let temp = TempDir::new().unwrap();
        let tree = temp.path().join("tree");
        fs::create_dir(&tree).unwrap();
        fs::create_dir(tree.join("nested")).unwrap();
        fs::write(tree.join("nested/file.txt"), "hello").unwrap();
        remove_directory_linux(&tree).unwrap();
        assert!(!tree.exists());
    }

    fn assert_copy_diverges_after_mutation(source: &Path, clone: &Path) {
        let original = fs::read_to_string(source).unwrap();
        assert_eq!(fs::read_to_string(clone).unwrap(), original);
        fs::write(source, "parent mutation").unwrap();
        assert_eq!(fs::read_to_string(clone).unwrap(), original);
        fs::write(clone, "child mutation").unwrap();
        assert_eq!(fs::read_to_string(source).unwrap(), "parent mutation");
        assert_eq!(fs::read_to_string(clone).unwrap(), "child mutation");
    }
}
