use anyhow::Result;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct CacheStats {
    pub archive_size: u64,
    pub archive_files: u64,
    pub aur_size: u64,
    pub aur_files: u64,
    pub total_size: u64,
}

pub struct CacheManager {
    store_root: PathBuf,
}

impl CacheManager {
    pub fn new(store_root: PathBuf) -> Self {
        Self { store_root }
    }

    pub fn status(&self) -> CacheStats {
        let archive_dir = self.store_root.join("cache/archive");
        let aur_dir = self.store_root.join("cache/aur");

        let (archive_size, archive_files) = Self::dir_size(&archive_dir);
        let (aur_size, aur_files) = Self::dir_size(&aur_dir);

        CacheStats {
            archive_size,
            archive_files,
            aur_size,
            aur_files,
            total_size: archive_size + aur_size,
        }
    }

    pub fn clean_archive(&self) -> Result<u64> {
        let dir = self.store_root.join("cache/archive");
        if !dir.exists() {
            return Ok(0);
        }

        let (size, _) = Self::dir_size(&dir);
        fs::remove_dir_all(&dir)?;
        fs::create_dir_all(&dir)?;
        println!("Cleaned archive cache: freed {}", Self::format_size(size));
        Ok(size)
    }

    pub fn clean_aur(&self) -> Result<u64> {
        let dir = self.store_root.join("cache/aur");
        if !dir.exists() {
            return Ok(0);
        }

        let (size, _) = Self::dir_size(&dir);
        fs::remove_dir_all(&dir)?;
        fs::create_dir_all(&dir)?;
        println!("Cleaned aur cache: freed {}", Self::format_size(size));
        Ok(size)
    }

    pub fn clean_all(&self) -> Result<u64> {
        let stats = self.status();
        let total = stats.total_size;

        self.clean_archive()?;
        self.clean_aur()?;

        println!("Cleaned all cache: freed {}", Self::format_size(total));
        Ok(total)
    }

    fn dir_size(path: &PathBuf) -> (u64, u64) {
        let mut size = 0u64;
        let mut files = 0u64;

        if !path.exists() {
            return (0, 0);
        }

        for entry in walkdir::WalkDir::new(path)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if entry.file_type().is_file() {
                size += entry.metadata().map(|m| m.len()).unwrap_or(0);
                files += 1;
            }
        }

        (size, files)
    }

    pub fn format_size(bytes: u64) -> String {
        const KB: u64 = 1024;
        const MB: u64 = KB * 1024;
        const GB: u64 = MB * 1024;

        if bytes >= GB {
            format!("{:.2} GB", bytes as f64 / GB as f64)
        } else if bytes >= MB {
            format!("{:.2} MB", bytes as f64 / MB as f64)
        } else if bytes >= KB {
            format!("{:.2} KB", bytes as f64 / KB as f64)
        } else {
            format!("{} B", bytes)
        }
    }
}
