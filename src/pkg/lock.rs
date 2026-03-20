use anyhow::{anyhow, Result};
use fs2::FileExt;
use std::fs::File;
use std::path::PathBuf;

#[derive(Debug)]
pub struct GlobalLock {
    file: File,
}

impl GlobalLock {
    pub fn acquire() -> Result<Self> {
        let uid = unsafe { libc::geteuid() };
        let lock_dir = PathBuf::from(format!("/run/user/{}/rscm", uid));
        std::fs::create_dir_all(&lock_dir)?;

        let file = File::create(lock_dir.join("lock"))?;
        file.try_lock_exclusive()
            .map_err(|_| anyhow!("Another rscm instance is running for this user"))?;

        Ok(Self { file })
    }

    pub fn try_acquire() -> Result<Option<Self>> {
        match Self::acquire() {
            Ok(lock) => Ok(Some(lock)),
            Err(_) if std::env::var("RSCM_NO_LOCK").is_ok() => Ok(None),
            Err(e) => Err(e),
        }
    }
}

impl Drop for GlobalLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}
