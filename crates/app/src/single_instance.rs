use anyhow::{Context, Result};
use fs2::FileExt;
use pipeweaver_smc_core::config::runtime_dir;
use std::fs::{self, File, OpenOptions};

pub struct InstanceLock {
    _file: File,
}

impl InstanceLock {
    pub fn acquire() -> Result<Option<Self>> {
        let dir = runtime_dir();
        fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
        let path = dir.join("pipeweaver-smc-mixer.lock");
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(&path)
            .with_context(|| format!("open {}", path.display()))?;
        match file.try_lock_exclusive() {
            Ok(()) => Ok(Some(Self { _file: file })),
            Err(_) => Ok(None),
        }
    }
}
