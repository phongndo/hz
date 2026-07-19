#[cfg(unix)]
use crate::Error;
use crate::Result;
use std::path::Path;

#[cfg(target_os = "linux")]
pub(super) fn ensure_no_mounted_descendants(path: &Path) -> Result<()> {
    let path = std::fs::canonicalize(path)?;
    let mountinfo = std::fs::read("/proc/self/mountinfo")?;
    if let Some(mount) = mounted_descendant_from_mountinfo(&path, &mountinfo) {
        Err(Error::MountedDescendant(mount))
    } else {
        Ok(())
    }
}

#[cfg(target_os = "linux")]
fn mounted_descendant_from_mountinfo(path: &Path, mountinfo: &[u8]) -> Option<std::path::PathBuf> {
    mount_points_from_mountinfo(mountinfo)
        .into_iter()
        .find(|mount| mount != path && mount.starts_with(path))
}

#[cfg(target_os = "linux")]
fn mount_points_from_mountinfo(mountinfo: &[u8]) -> Vec<std::path::PathBuf> {
    use std::os::unix::ffi::OsStringExt;

    mountinfo
        .split(|byte| *byte == b'\n')
        .filter_map(|line| line.split(|byte| *byte == b' ').nth(4))
        .map(|field| {
            let mut decoded = Vec::with_capacity(field.len());
            let mut index = 0;
            while index < field.len() {
                if field[index] == b'\\'
                    && index + 3 < field.len()
                    && field[index + 1..=index + 3]
                        .iter()
                        .all(|byte| (b'0'..=b'7').contains(byte))
                {
                    let value = u16::from(field[index + 1] - b'0') * 64
                        + u16::from(field[index + 2] - b'0') * 8
                        + u16::from(field[index + 3] - b'0');
                    if let Ok(value) = u8::try_from(value) {
                        decoded.push(value);
                        index += 4;
                        continue;
                    }
                }
                decoded.push(field[index]);
                index += 1;
            }
            std::path::PathBuf::from(std::ffi::OsString::from_vec(decoded))
        })
        .collect()
}

#[cfg(target_os = "macos")]
pub(super) fn ensure_no_mounted_descendants(path: &Path) -> Result<()> {
    use std::ffi::OsStr;
    use std::mem::{MaybeUninit, size_of};
    use std::os::unix::ffi::OsStrExt;

    let path = std::fs::canonicalize(path)?;
    // SAFETY: A null buffer asks macOS for the current mount count.
    let mount_count = unsafe { libc::getfsstat(std::ptr::null_mut(), 0, libc::MNT_NOWAIT) };
    if mount_count < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let mut capacity = mount_count as usize + 1;
    loop {
        let Some(buffer_size) = capacity
            .checked_mul(size_of::<libc::statfs>())
            .and_then(|size| libc::c_int::try_from(size).ok())
        else {
            return Err(std::io::Error::other("macOS mount table is too large").into());
        };
        let mut mount_buffer = Vec::with_capacity(capacity);
        mount_buffer.resize_with(capacity, MaybeUninit::<libc::statfs>::uninit);
        // SAFETY: The buffer has room for `buffer_size` bytes of correctly
        // aligned `statfs` entries, which macOS initializes before returning.
        let mount_count = unsafe {
            libc::getfsstat(
                mount_buffer.as_mut_ptr().cast(),
                buffer_size,
                libc::MNT_NOWAIT,
            )
        };
        if mount_count < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        let mount_count = mount_count as usize;
        if mount_count >= capacity {
            capacity = capacity
                .checked_mul(2)
                .and_then(|capacity| capacity.checked_add(1))
                .ok_or_else(|| std::io::Error::other("macOS mount table is too large"))?;
            continue;
        }
        for mount in &mount_buffer[..mount_count] {
            // SAFETY: `getfsstat` initialized every entry below its returned count.
            let mount = unsafe { mount.assume_init_ref() };
            let path_length = mount
                .f_mntonname
                .iter()
                .position(|byte| *byte == 0)
                .unwrap_or(mount.f_mntonname.len());
            let bytes = mount.f_mntonname[..path_length]
                .iter()
                .map(|byte| *byte as u8)
                .collect::<Vec<_>>();
            let mount = std::path::PathBuf::from(OsStr::from_bytes(&bytes));
            if mount != path && mount.starts_with(&path) {
                return Err(Error::MountedDescendant(mount));
            }
        }
        return Ok(());
    }
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
pub(super) fn ensure_no_mounted_descendants(path: &Path) -> Result<()> {
    use std::os::unix::fs::MetadataExt;

    let path = std::fs::canonicalize(path)?;
    let device = std::fs::metadata(&path)?.dev();
    for entry in walkdir::WalkDir::new(&path)
        .min_depth(1)
        .follow_links(false)
    {
        let entry = entry?;
        if entry.file_type().is_dir() && std::fs::metadata(entry.path())?.dev() != device {
            return Err(Error::MountedDescendant(entry.path().to_path_buf()));
        }
    }
    Ok(())
}

#[cfg(not(unix))]
pub(super) fn ensure_no_mounted_descendants(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn mountinfo_parser_detects_escaped_descendant_mounts() {
        let mountinfo = b"1 0 0:1 / / rw - rootfs rootfs rw\n\
                          2 1 0:2 / /workspace/bind\\040mount rw - tmpfs tmpfs rw\n";

        assert_eq!(
            mounted_descendant_from_mountinfo(Path::new("/workspace"), mountinfo),
            Some(std::path::PathBuf::from("/workspace/bind mount"))
        );
    }

    #[test]
    fn mountinfo_parser_does_not_treat_the_root_mount_as_a_descendant() {
        let mountinfo = b"1 0 0:1 / /workspace rw - rootfs rootfs rw\n";

        assert_eq!(
            mounted_descendant_from_mountinfo(Path::new("/workspace"), mountinfo),
            None
        );
    }
}
