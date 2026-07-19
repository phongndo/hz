use std::{fs, path::Path};

use fs2::FileExt;

use crate::Result;

pub(crate) struct MutationLock {
    file: fs::File,
}

impl MutationLock {
    pub(crate) fn acquire(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut options = fs::OpenOptions::new();
        options.read(true).write(true).create(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(path)?;
        file.lock_exclusive()?;
        Ok(Self { file })
    }
}

impl Drop for MutationLock {
    fn drop(&mut self) {
        let _ = fs2::FileExt::unlock(&self.file);
    }
}
