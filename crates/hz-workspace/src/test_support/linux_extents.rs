use crate::{Error, Result};
use std::fs::File;
use std::os::fd::AsRawFd;
use std::path::Path;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LinuxFilesystem {
    Btrfs,
    Xfs,
    Zfs,
    Other,
}

pub(crate) fn filesystem(path: &Path) -> Result<LinuxFilesystem> {
    use std::os::unix::ffi::OsStrExt;

    const BTRFS_SUPER_MAGIC: libc::c_long = 0x9123_683e;
    const XFS_SUPER_MAGIC: libc::c_long = 0x5846_5342;
    const ZFS_SUPER_MAGIC: libc::c_long = 0x2fc1_2fc1;

    let path = std::ffi::CString::new(path.as_os_str().as_bytes())
        .map_err(|_| Error::Path(format!("path contains a null byte: {}", path.display())))?;
    // SAFETY: `statfs` is a plain C struct that the kernel fully initializes.
    let mut stat: libc::statfs = unsafe { std::mem::zeroed() };
    // SAFETY: `path` is a valid C string, and `stat` points to writable memory.
    if unsafe { libc::statfs(path.as_ptr(), &mut stat) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(match stat.f_type {
        BTRFS_SUPER_MAGIC => LinuxFilesystem::Btrfs,
        XFS_SUPER_MAGIC => LinuxFilesystem::Xfs,
        ZFS_SUPER_MAGIC => LinuxFilesystem::Zfs,
        _ => LinuxFilesystem::Other,
    })
}

pub(crate) fn assert_shared_extents_when_reliable(source: &Path, clone: &Path) {
    match (
        filesystem(source).unwrap(),
        have_shared_extents(source, clone),
    ) {
        (_, Ok(true)) => {}
        (LinuxFilesystem::Xfs, Ok(false)) => {
            panic!(
                "expected FIEMAP shared extents for XFS reflink copy: {} -> {}",
                source.display(),
                clone.display()
            );
        }
        (LinuxFilesystem::Xfs, Err(error)) => {
            panic!(
                "expected readable FIEMAP shared extents for XFS reflink copy: {} -> {}: {error}",
                source.display(),
                clone.display()
            );
        }
        (_, Ok(false) | Err(_)) => {}
    }
}

fn have_shared_extents(source: &Path, clone: &Path) -> std::io::Result<bool> {
    let source = file_extents(source)?;
    let clone = file_extents(clone)?;

    Ok(source.iter().any(|source| {
        source.is_shared()
            && clone
                .iter()
                .any(|clone| clone.is_shared() && source.physical_range_overlaps(clone))
    }))
}

fn file_extents(path: &Path) -> std::io::Result<Vec<FiemapExtent>> {
    const FS_IOC_FIEMAP: libc::c_ulong = 0xc020_660b;
    const FIEMAP_FLAG_SYNC: u32 = 0x0000_0001;
    const EXTENT_COUNT: u32 = 32;

    let file = File::open(path)?;
    let mut request = FiemapRequest {
        map: Fiemap {
            fm_start: 0,
            fm_length: u64::MAX,
            fm_flags: FIEMAP_FLAG_SYNC,
            fm_mapped_extents: 0,
            fm_extent_count: EXTENT_COUNT,
            fm_reserved: 0,
        },
        extents: [FiemapExtent::default(); EXTENT_COUNT as usize],
    };

    // SAFETY: `file` is open for the duration of the call, and `request`
    // matches the Linux FIEMAP layout with room for `fm_extent_count` extents.
    if unsafe { libc::ioctl(file.as_raw_fd(), FS_IOC_FIEMAP, &mut request) } != 0 {
        return Err(std::io::Error::last_os_error());
    }

    let mapped_extents = request.map.fm_mapped_extents.min(EXTENT_COUNT) as usize;
    Ok(request.extents[..mapped_extents].to_vec())
}

#[repr(C)]
struct Fiemap {
    fm_start: u64,
    fm_length: u64,
    fm_flags: u32,
    fm_mapped_extents: u32,
    fm_extent_count: u32,
    fm_reserved: u32,
}

#[repr(C)]
struct FiemapRequest {
    map: Fiemap,
    extents: [FiemapExtent; 32],
}

#[derive(Clone, Copy, Default)]
#[repr(C)]
struct FiemapExtent {
    fe_logical: u64,
    fe_physical: u64,
    fe_length: u64,
    fe_reserved64: [u64; 2],
    fe_flags: u32,
    fe_reserved: [u32; 3],
}

impl FiemapExtent {
    fn is_shared(self) -> bool {
        const FIEMAP_EXTENT_SHARED: u32 = 0x0000_2000;

        self.fe_flags & FIEMAP_EXTENT_SHARED != 0
    }

    fn physical_range_overlaps(self, other: &Self) -> bool {
        let Some(end) = self.fe_physical.checked_add(self.fe_length) else {
            return false;
        };
        let Some(other_end) = other.fe_physical.checked_add(other.fe_length) else {
            return false;
        };

        self.fe_length > 0
            && other.fe_length > 0
            && self.fe_physical < other_end
            && other.fe_physical < end
    }
}
