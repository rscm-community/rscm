use crate::config::Configuration;
use crate::pkg::{PackageConfig, PackageInfo, PackageManagerFactory};
use anyhow::{anyhow, Result};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone)]
pub struct LockedPackage {
    pub name: String,
    pub version: String,
    pub release: String,
    pub source: String,
    pub hash: String,
    pub dependencies: Vec<String>,
}

impl LockedPackage {
    pub fn from_info(info: &PackageInfo) -> Self {
        let hash = Sha256::digest(format!(
            "{}-{}-{}-{}",
            info.name,
            info.version,
            info.release,
            info.source.as_str()
        ));
        let hash_str = format!("sha256:{}", hex::encode(&hash));

        Self {
            name: info.name.clone(),
            version: info.version.clone(),
            release: info.release.clone(),
            source: info.source.as_str().to_string(),
            hash: hash_str,
            dependencies: info.dependencies.clone(),
        }
    }

    pub fn compute_content_hash(&self) -> String {
        let hash = Sha256::digest(format!(
            "{}\n{}\n{}\n{}",
            self.name, self.version, self.release, self.source
        ));
        hex::encode(&hash)
    }
}

pub struct Resolver {
    factory: PackageManagerFactory,
    resolved: HashMap<String, LockedPackage>,
    by_name: HashMap<String, Vec<String>>,
    pending: HashSet<String>,
    provides_map: HashMap<String, String>,
}

#[derive(Clone, Debug)]
struct PackageKey {
    name: String,
    version: Option<String>,
}

impl Resolver {
    pub fn new(store_root: std::path::PathBuf) -> Self {
        Self {
            factory: PackageManagerFactory::new(store_root),
            resolved: HashMap::new(),
            by_name: HashMap::new(),
            pending: HashSet::new(),
            provides_map: HashMap::new(),
        }
    }

    pub fn resolve_config(
        &mut self,
        config: &Configuration,
    ) -> Result<HashMap<String, Vec<LockedPackage>>> {
        self.resolved.clear();
        self.by_name.clear();
        self.pending.clear();

        let packages = self.collect_packages(config);
        for pkg in packages {
            self.resolve_package(&pkg.name, pkg.version.as_deref())?;
        }

        let mut result: HashMap<String, Vec<LockedPackage>> = HashMap::new();
        for (key, pkg) in &self.resolved {
            let name = pkg.name.clone();
            result
                .entry(name)
                .or_insert_with(Vec::new)
                .push(pkg.clone());
        }
        Ok(result)
    }

    fn collect_packages(&self, config: &Configuration) -> Vec<PackageKey> {
        let mut packages = Vec::new();
        let mut seen = HashSet::new();

        for name in &config.packages.list {
            if seen.insert(name.clone()) {
                packages.push(PackageKey {
                    name: name.clone(),
                    version: None,
                });
            }
        }

        for (name, opts) in &config.packages.map {
            if let Some(versions) = &opts.versions {
                for (ver_key, ver_opts) in versions {
                    if seen.insert(format!("{}:{}", name, ver_key)) {
                        packages.push(PackageKey {
                            name: name.clone(),
                            version: Some(ver_opts.version.clone()),
                        });
                    }
                }
            } else {
                if seen.insert(name.clone()) {
                    packages.push(PackageKey {
                        name: name.clone(),
                        version: opts.version.clone(),
                    });
                }
            }
            for dep in &opts.dependencies {
                let dep_name = Self::normalize_dep_name(dep);
                if seen.insert(dep_name.clone()) {
                    packages.push(PackageKey {
                        name: dep_name,
                        version: None,
                    });
                }
            }
        }

        if let Some(ref boot) = config.boot {
            if let Some(ref kernel) = boot.kernel {
                if let Some(ref pkg) = kernel.package {
                    if seen.insert(pkg.clone()) {
                        packages.push(PackageKey {
                            name: pkg.clone(),
                            version: None,
                        });
                    }
                }
                if let Some(ref pkgs) = kernel.packages {
                    for pkg_name in pkgs {
                        if seen.insert(pkg_name.clone()) {
                            packages.push(PackageKey {
                                name: pkg_name.clone(),
                                version: None,
                            });
                        }
                    }
                }
            }
        }

        packages
    }

    pub fn resolve_package(&mut self, name: &str, version: Option<&str>) -> Result<LockedPackage> {
        let key = format!("{}:{}", name, version.unwrap_or("*"));

        if self.pending.contains(&key) {
            return Ok(self.create_placeholder_package(name));
        }

        self.pending.insert(key.clone());

        let resolve_name = self.resolve_provides_name(name)?;
        let info = self.query_package(&resolve_name, version)?;

        for prov in &info.provides {
            let prov_clean = Self::normalize_dep_name(prov);
            self.provides_map
                .entry(prov_clean)
                .or_insert_with(|| info.name.clone());
        }

        let actual_key = format!("{}:{}", info.name, info.version);

        if let Some(existing) = self.resolved.get(&actual_key) {
            self.pending.remove(&key);
            return Ok(existing.clone());
        }

        for dep in &info.dependencies {
            let dep_clean = Self::normalize_dep_name(dep);
            if !self
                .resolved
                .keys()
                .any(|k| k.starts_with(&format!("{}:", dep_clean)))
            {
                if let Err(e) = self.resolve_package(&dep_clean, None) {
                    eprintln!("Warning: skipping dependency {}: {}", dep_clean, e);
                }
            }
        }

        self.pending.remove(&key);

        let locked = LockedPackage::from_info(&info);
        self.resolved.insert(actual_key, locked.clone());
        self.by_name
            .entry(info.name.clone())
            .or_insert_with(Vec::new)
            .push(format!("{}:{}", info.name, info.version));

        Ok(locked)
    }

    fn create_placeholder_package(&self, name: &str) -> LockedPackage {
        LockedPackage {
            name: name.to_string(),
            version: "0.0.0".to_string(),
            release: "0".to_string(),
            source: "unknown".to_string(),
            hash: "sha256:placeholder".to_string(),
            dependencies: vec![],
        }
    }

    fn query_package(&self, name: &str, version: Option<&str>) -> Result<PackageInfo> {
        let package_config = PackageConfig {
            name: name.to_string(),
            version: version.map(String::from),
            build_type: crate::pkg::BuildType::Pacman,
            dependencies: vec![],
            sandbox_config: None,
        };

        if let Ok(pacman) = self.factory.for_package(&package_config) {
            println!(
                "Querying package info for {}, using pacman (version: {:?})",
                name, version
            );
            if let Some(info) = pacman.query_package_info(name, version)? {
                return Ok(info);
            }
        }

        if let Some(aur) = self.factory.aur_manager() {
            println!(
                "Querying package info for {}, using AUR (version: {:?})",
                name, version
            );
            if let Some(info) = aur.query_package_info(name, version)? {
                return Ok(info);
            }
        }

        if let Some(info) = self.factory.pacman().find_package_by_provides(name)? {
            return Ok(info);
        }

        Err(anyhow!("Package not found: {}", name))
    }

    fn normalize_dep_name(dep: &str) -> String {
        dep.split(|c: char| c == '<' || c == '>' || c == '=' || c == '=' || c == ' ')
            .next()
            .unwrap_or(dep)
            .trim()
            .to_string()
    }

    fn resolve_provides_name(&self, name: &str) -> Result<String> {
        let normalized = Self::normalize_dep_name(name);
        if let Some(real_pkg) = self.provides_map.get(&normalized) {
            return Ok(real_pkg.clone());
        }
        Ok(normalized)
    }

    pub fn resolve_incremental(
        &mut self,
        _current: &HashMap<String, Vec<LockedPackage>>,
        new_packages: &[String],
    ) -> Result<HashMap<String, Vec<LockedPackage>>> {
        self.resolved.clear();
        self.by_name.clear();

        for pkg_name in new_packages {
            self.resolve_package(pkg_name, None)?;
        }

        let mut result: HashMap<String, Vec<LockedPackage>> = HashMap::new();
        for (key, pkg) in &self.resolved {
            let name = pkg.name.clone();
            result
                .entry(name)
                .or_insert_with(Vec::new)
                .push(pkg.clone());
        }
        Ok(result)
    }
}
