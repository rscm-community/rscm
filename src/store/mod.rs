pub mod content;
pub mod generation;
pub mod package;
pub mod reference;

pub use content::ContentStore;
pub use generation::{Generation, GenerationStore};
pub use package::{Package, PackageStore};
pub use reference::ReferenceCounter;

use anyhow::Result;
use std::path::{Path, PathBuf};

pub struct Store {
    root: PathBuf,
    content: ContentStore,
    packages: PackageStore,
    generations: GenerationStore,
    reference: ReferenceCounter,
}

impl Store {
    pub fn new(root: PathBuf) -> Result<Self> {
        if !root.exists() {
            anyhow::bail!(
                "Store directory {} does not exist. Run 'rscm init' first to initialize storage.",
                root.display()
            );
        }
        let content = ContentStore::new(root.join("content"))?;
        let packages = PackageStore::new(root.join("packages"))?;
        let generations = GenerationStore::new(root.join("generations"))?;
        let reference = ReferenceCounter::new(root.join("references.json"))?;

        Ok(Self {
            root,
            content,
            packages,
            generations,
            reference,
        })
    }
    pub fn register_package(&mut self, pkg: Package) -> Result<()> {
        self.packages.save(&pkg)?;
        for file in &pkg.files {
            self.reference.add(&file.hash)?;
        }
        Ok(())
    }

    pub fn create_generation(&mut self, package_names: &[String]) -> Result<u64> {
        let mut files = Vec::new();
        for name in package_names {
            if let Some(pkg) = self.packages.get(name)? {
                for file in &pkg.files {
                    files.push(file.clone());
                }
            }
        }
        let id = self.generations.create(package_names, &files, |src, dst| {
            self.content.link_to(src, dst)
        })?;

        Ok(id)
    }

    pub fn activate_generation(&self, id: u64) -> Result<()> {
        let gen_path = self.generations.path(id);
        let current = Path::new("/rscm/current-system");
        if current.exists() {
            std::fs::remove_file(current)?;
        }
        std::os::unix::fs::symlink(gen_path, current)?;
        Ok(())
    }
}
