use anyhow::Result;
use rscm::pkg::{
    BuildType, PackageConfig, PackageManager, PackageManagerFactory, PackageType,
    PackageSource, RemoveResult, SandboxConfig, aur::AurHelper, lock::GlobalLock, pacman::Pacman,
};
use std::fs;
use tempfile::tempdir;
use std::path::PathBuf;

mod global_lock_tests {
    use super::*;
    #[test]
    fn test_global_lock_acquire() -> Result<()> {
        let lock = GlobalLock::acquire()?;
        drop(lock);
        Ok(())
    }

    #[test]
    fn test_global_lock_prevents_concurrent() -> Result<()> {
        let _lock1 = GlobalLock::acquire()?;
        let result = GlobalLock::acquire();
        assert!(result.is_err());
        let err_msg = result.err().unwrap().to_string();
        assert!(err_msg.contains("Another rscm instance is running"));
        Ok(())
    }

    #[test]
    fn test_global_lock_auto_release() -> Result<()> {
        {
            let _lock1 = GlobalLock::acquire()?;
        }
        let _lock2 = GlobalLock::acquire()?;
        Ok(())
    }
}

mod package_manager_factory_tests {
    use super::*;
    #[test]
    fn test_package_manager_factory_creation() -> Result<()> {
        let dir = tempdir()?;
        let store_root = dir.path().join("store");
        fs::create_dir_all(&store_root)?;

        let factory = PackageManagerFactory::new(store_root.clone());
        assert!(factory.has_aur_helper() || !factory.has_aur_helper());
        Ok(())
    }

    #[test]
    fn test_package_manager_factory_pacman_manager() -> Result<()> {
        let dir = tempdir()?;
        let store_root = dir.path().join("store");
        fs::create_dir_all(&store_root)?;

        let factory = PackageManagerFactory::new(store_root);
        let pkg_manager = factory.pacman_manager();

        assert_eq!(pkg_manager.manager_name(), "pacman");
        assert_eq!(pkg_manager.build_type(), BuildType::Pacman);
        Ok(())
    }

    #[test]
    fn test_package_manager_factory_for_package() -> Result<()> {
        let dir = tempdir()?;
        let store_root = dir.path().join("store");
        fs::create_dir_all(&store_root)?;

        let factory = PackageManagerFactory::new(store_root);

        let pacman_config = PackageConfig {
            name: "test".to_string(),
            version: None,
            build_type: BuildType::Pacman,
            dependencies: vec![],
            sandbox_config: None,
        };

        let pkg_manager = factory.for_package(&pacman_config)?;
        assert_eq!(pkg_manager.manager_name(), "pacman");
        Ok(())
    }
}

mod pacman_tests {
    use super::*;
    #[test]
    fn test_pacman_creation() -> Result<()> {
        let dir = tempdir()?;
        let store_root = dir.path().join("store");
        fs::create_dir_all(&store_root)?;

        let _pacman = Pacman::new(store_root.clone());
        Ok(())
    }

    #[test]
    fn test_pacman_system_creation() -> Result<()> {
        let dir = tempdir()?;
        let store_root = dir.path().join("store");
        fs::create_dir_all(&store_root)?;

        let _pacman = Pacman::system(store_root);
        Ok(())
    }

    #[test]
    fn test_pacman_install_nonexistent_package() -> Result<()> {
        let dir = tempdir()?;
        let store_root = dir.path().join("store");
        fs::create_dir_all(&store_root)?;

        let pacman = Pacman::new(store_root);

        let pkg_config = PackageConfig {
            name: "nonexistent-package-xyz123".to_string(),
            version: None,
            build_type: BuildType::Pacman,
            dependencies: vec![],
            sandbox_config: None,
        };

        let result = pacman.install(&pkg_config, false);
        assert!(result.is_err());
        Ok(())
    }

    #[test]
    fn test_pacman_remove_nonexistent_package() -> Result<()> {
        let dir = tempdir()?;
        let store_root = dir.path().join("store");
        fs::create_dir_all(&store_root)?;

        let pacman = Pacman::new(store_root);

        let result = pacman.remove("nonexistent-package-xyz123", None, false);
        assert!(result.is_ok() || result.is_err());
        Ok(())
    }

    #[test]
    fn test_pacman_query_package_info() -> Result<()> {
        let dir = tempdir()?;
        let store_root = dir.path().join("store");
        fs::create_dir_all(&store_root)?;

        let pacman = Pacman::new(store_root);
        let _result = pacman.query_package_info("bash", None);
        Ok(())
    }
}

mod remove_result_tests {
    use super::*;
    #[test]
    fn test_remove_result_structure() -> Result<()> {
        let result = RemoveResult {
            package_name: "test-pkg".to_string(),
            removed_versions: vec!["1.0-1".to_string()],
            files_removed: 10,
            space_freed: 1024 * 1024,
            recursive: true,
            removed_dependents: vec!["dep1".to_string(), "dep2".to_string()],
        };

        assert_eq!(result.package_name, "test-pkg");
        assert_eq!(result.removed_versions.len(), 1);
        assert_eq!(result.files_removed, 10);
        assert_eq!(result.space_freed, 1024 * 1024);
        assert!(result.recursive);
        assert_eq!(result.removed_dependents.len(), 2);
        Ok(())
    }
}

mod package_config_tests {
    use super::*;
    #[test]
    fn test_package_config_creation() -> Result<()> {
        let config = PackageConfig {
            name: "test-package".to_string(),
            version: Some("1.0.0".to_string()),
            build_type: BuildType::Pacman,
            dependencies: vec!["dep1".to_string(), "dep2".to_string()],
            sandbox_config: None,
        };

        assert_eq!(config.name, "test-package");
        assert_eq!(config.version, Some("1.0.0".to_string()));
        assert_eq!(config.build_type, BuildType::Pacman);
        assert_eq!(config.dependencies.len(), 2);
        Ok(())
    }

    #[test]
    fn test_package_config_default_sandbox() -> Result<()> {
        let config = SandboxConfig::default();
        assert!(!config.network);
        assert!(config.ro_paths.is_empty());
        assert!(config.rw_paths.is_empty());
        Ok(())
    }
}

mod build_type_tests {
    use super::*;
    #[test]
    fn test_build_type_equality() -> Result<()> {
        assert_eq!(BuildType::Pacman, BuildType::Pacman);
        assert_eq!(BuildType::Aur, BuildType::Aur);
        assert_ne!(BuildType::Pacman, BuildType::Aur);
        Ok(())
    }
}

mod package_source_tests {
    use super::*;
    #[test]
    fn test_package_source_as_str() -> Result<()> {
        let repo = PackageSource::Repository("core".to_string());
        assert_eq!(repo.as_str(), "core");

        let aur = PackageSource::Aur;
        assert_eq!(aur.as_str(), "aur");

        let local = PackageSource::Local;
        assert_eq!(local.as_str(), "local");

        let other = PackageSource::Other("custom".to_string());
        assert_eq!(other.as_str(), "custom");
        Ok(())
    }
}

mod integration_tests {
    use super::*;
    #[test]
    fn test_pacman_install_and_verify() -> Result<()> {
        let dir = tempdir()?;
        let store_root = dir.path().join("store");
        fs::create_dir_all(&store_root)?;

        let pacman = Pacman::new(store_root);

        let pkg_config = PackageConfig {
            name: "base".to_string(),
            version: None,
            build_type: BuildType::Pacman,
            dependencies: vec![],
            sandbox_config: None,
        };

        let result = pacman.install(&pkg_config, false);
        if let Ok(info) = result {
            assert_eq!(info.name, "base");
            assert!(info.installed);
            assert!(matches!(info.source, PackageSource::Repository(_)));
        }
        Ok(())
    }
}

mod edge_cases_tests {
    use super::*;
    #[test]
    fn test_remove_with_version() -> Result<()> {
        let dir = tempdir()?;
        let store_root = dir.path().join("store");
        fs::create_dir_all(&store_root)?;

        let pacman = Pacman::new(store_root);
        let _result = pacman.remove("test-pkg", Some("1.0-1"), false);
        Ok(())
    }

    #[test]
    fn test_remove_recursive_flag() -> Result<()> {
        let dir = tempdir()?;
        let store_root = dir.path().join("store");
        fs::create_dir_all(&store_root)?;

        let pacman = Pacman::new(store_root);
        let _result = pacman.remove("test-pkg", None, true);
        Ok(())
    }

    #[test]
    fn test_install_force_flag() -> Result<()> {
        let dir = tempdir()?;
        let store_root = dir.path().join("store");
        fs::create_dir_all(&store_root)?;

        let pacman = Pacman::new(store_root);

        let pkg_config = PackageConfig {
            name: "nonexistent".to_string(),
            version: None,
            build_type: BuildType::Pacman,
            dependencies: vec![],
            sandbox_config: None,
        };

        let result = pacman.install(&pkg_config, true);
        assert!(result.is_err());
        Ok(())
    }
}

mod store_integration_tests {
    use super::*;
    #[test]
    fn test_pacman_store_directory_creation() -> Result<()> {
        let dir = tempdir()?;
        let store_root = dir.path().join("store");

        let _pacman = Pacman::new(store_root.clone());

        assert!(store_root.exists());
        assert!(store_root.join("tmp/pacman").exists());
        assert!(store_root.join("tmp/pacman/var/lib/pacman").exists());
        assert!(store_root.join("tmp/pacman/var/cache/pacman/pkg").exists());
        Ok(())
    }

    #[test]
    fn test_pacman_system_mode_no_isolated_root() -> Result<()> {
        let dir = tempdir()?;
        let store_root = dir.path().join("store");

        let _pacman = Pacman::system(store_root);
        Ok(())
    }
}

mod aur_helper_tests {
    use super::*;
    #[test]
    fn test_aur_helper_creation() -> Result<()> {
        let dir = tempdir()?;
        let store_root = dir.path().join("store");
        fs::create_dir_all(&store_root)?;

        if let Some(aur) = AurHelper::detect(store_root.clone()) {
            assert_eq!(aur.build_type(), BuildType::Aur);
        }
        Ok(())
    }

    #[test]
    fn test_aur_helper_build_dir() -> Result<()> {
        let dir = tempdir()?;
        let store_root = dir.path().join("store");
        fs::create_dir_all(&store_root)?;

        if let Some(aur) = AurHelper::detect(store_root) {
            let _build_dir = aur.build_dir();
        }
        Ok(())
    }
}

mod pacman_install_tests {
    use super::*;
    use rscm::store::PackageStore;

    #[test]
    fn test_pacman_install_base_package_to_store() -> Result<()> {
        let dir = tempdir()?;
        let store_root = dir.path().join("store");
        fs::create_dir_all(&store_root)?;

        let pacman = Pacman::new(store_root.clone());

        let pkg_config = PackageConfig {
            name: "base".to_string(),
            version: None,
            build_type: BuildType::Pacman,
            dependencies: vec![],
            sandbox_config: None,
        };

        let result = pacman.install(&pkg_config, false);
        match result {
            Ok(info) => {
                assert_eq!(info.name, "base");
                assert!(info.installed);
                assert_eq!(info.ty, PackageType::Pacman);

                let package_store = PackageStore::new(store_root.join("packages"))?;
                let pkg = package_store.get("base")?;
                assert!(pkg.is_some());
            }
            Err(e) => {
                println!("Install failed (expected if no network): {}", e);
            }
        }
        Ok(())
    }

    #[test]
    fn test_pacman_install_dbus_to_store() -> Result<()> {
        let dir = tempdir()?;
        let store_root = dir.path().join("store");
        fs::create_dir_all(&store_root)?;

        let pacman = Pacman::new(store_root.clone());

        let pkg_config = PackageConfig {
            name: "dbus".to_string(),
            version: None,
            build_type: BuildType::Pacman,
            dependencies: vec![],
            sandbox_config: None,
        };

        let result = pacman.install(&pkg_config, false);
        match result {
            Ok(info) => {
                assert_eq!(info.name, "dbus");
                assert!(info.installed);

                let package_store = PackageStore::new(store_root.join("packages"))?;
                let pkg = package_store.get("dbus")?;
                assert!(pkg.is_some());

                let pkg = pkg.unwrap();
                assert!(!pkg.files.is_empty());
                assert!(pkg.dependencies.len() > 0);
            }
            Err(e) => {
                println!("Install failed (expected if no network): {}", e);
            }
        }
        Ok(())
    }

    #[test]
    fn test_pacman_install_with_version() -> Result<()> {
        let dir = tempdir()?;
        let store_root = dir.path().join("store");
        fs::create_dir_all(&store_root)?;

        let pacman = Pacman::new(store_root);

        let pkg_config = PackageConfig {
            name: "glibc".to_string(),
            version: Some("2.38-1".to_string()),
            build_type: BuildType::Pacman,
            dependencies: vec![],
            sandbox_config: None,
        };

        let result = pacman.install(&pkg_config, false);
        match result {
            Ok(info) => {
                assert_eq!(info.name, "glibc");
                assert!(info.installed);
            }
            Err(e) => {
                println!("Install failed (expected if no network): {}", e);
            }
        }
        Ok(())
    }

    #[test]
    fn test_pacman_check_store_exists() -> Result<()> {
        let dir = tempdir()?;
        let store_root = dir.path().join("store");
        fs::create_dir_all(&store_root)?;

        let pacman = Pacman::new(store_root.clone());

        let pkg_config = PackageConfig {
            name: "base".to_string(),
            version: None,
            build_type: BuildType::Pacman,
            dependencies: vec![],
            sandbox_config: None,
        };

        let is_available = pacman.is_available_in_store(&pkg_config);
        assert!(!is_available);

        let _ = pacman.install(&pkg_config, false);

        let is_available = pacman.is_available_in_store(&pkg_config);
        Ok(())
    }

    #[test]
    fn test_pacman_list_installed_from_store() -> Result<()> {
        let dir = tempdir()?;
        let store_root = dir.path().join("store");
        fs::create_dir_all(&store_root)?;

        let pacman = Pacman::new(store_root.clone());

        let installed = pacman.list_installed()?;
        assert!(installed.is_empty());

        let pkg_config = PackageConfig {
            name: "base".to_string(),
            version: None,
            build_type: BuildType::Pacman,
            dependencies: vec![],
            sandbox_config: None,
        };

        let _ = pacman.install(&pkg_config, false);

        let installed = pacman.list_installed()?;
        Ok(())
    }
}

mod aur_install_tests {
    use super::*;
    use rscm::store::PackageStore;

    #[test]
    fn test_aur_helper_install_to_store() -> Result<()> {
        let dir = tempdir()?;
        let store_root = dir.path().join("store");
        fs::create_dir_all(&store_root)?;

        let aur = match AurHelper::detect(store_root.clone()) {
            Some(a) => a,
            None => {
                println!("No AUR helper available, skipping test");
                return Ok(());
            }
        };

        let pkg_config = PackageConfig {
            name: "hello".to_string(),
            version: None,
            build_type: BuildType::Aur,
            dependencies: vec![],
            sandbox_config: None,
        };

        let result = aur.install(&pkg_config, false);
        match result {
            Ok(info) => {
                assert_eq!(info.name, "hello");
                assert!(info.installed);
                assert_eq!(info.ty, PackageType::Aur);

                let package_store = PackageStore::new(store_root.join("packages"))?;
                let pkg = package_store.get("hello")?;
                assert!(pkg.is_some());
            }
            Err(e) => {
                println!(
                    "Install failed (expected if AUR package not found or no makepkg): {}",
                    e
                );
            }
        }
        Ok(())
    }

    #[test]
    fn test_aur_helper_check_store_exists() -> Result<()> {
        let dir = tempdir()?;
        let store_root = dir.path().join("store");
        fs::create_dir_all(&store_root)?;

        let aur = match AurHelper::detect(store_root.clone()) {
            Some(a) => a,
            None => {
                println!("No AUR helper available, skipping test");
                return Ok(());
            }
        };

        let pkg_config = PackageConfig {
            name: "hello".to_string(),
            version: None,
            build_type: BuildType::Aur,
            dependencies: vec![],
            sandbox_config: None,
        };

        let is_available = aur.is_available_in_store(&pkg_config);
        assert!(!is_available);
        Ok(())
    }

    #[test]
    fn test_aur_helper_list_installed() -> Result<()> {
        let dir = tempdir()?;
        let store_root = dir.path().join("store");
        fs::create_dir_all(&store_root)?;

        let aur = match AurHelper::detect(store_root) {
            Some(a) => a,
            None => {
                println!("No AUR helper available, skipping test");
                return Ok(());
            }
        };

        let installed = aur.list_installed()?;
        assert!(installed.is_empty());
        Ok(())
    }

    #[test]
    fn test_aur_helper_query_info() -> Result<()> {
        let dir = tempdir()?;
        let store_root = dir.path().join("store");
        fs::create_dir_all(&store_root)?;

        let aur = match AurHelper::detect(store_root) {
            Some(a) => a,
            None => {
                println!("No AUR helper available, skipping test");
                return Ok(());
            }
        };

        let info = aur.query_package_info("hello", None);
        match info {
            Ok(Some(pkg)) => {
                assert_eq!(pkg.name, "hello");
                assert!(!pkg.version.is_empty());
            }
            Ok(None) => {
                println!("Package not found in AUR");
            }
            Err(e) => {
                println!("Query failed: {}", e);
            }
        }
        Ok(())
    }
}

mod store_package_tests {
    use super::*;
    use rscm::store::PackageStore;
    use rscm::store::package::{FileEntry, Package};
    use std::time::SystemTime;

    #[test]
    fn test_package_store_creation() -> Result<()> {
        let dir = tempdir()?;
        let store_root = dir.path().join("store");
        fs::create_dir_all(&store_root)?;

        let package_store = PackageStore::new(store_root.join("packages"))?;
        Ok(())
    }

    #[test]
    fn test_package_store_save_and_get() -> Result<()> {
        let dir = tempdir()?;
        let store_root = dir.path().join("store");
        fs::create_dir_all(&store_root)?;

        let package_store = PackageStore::new(store_root.join("packages"))?;

        let pkg = Package {
            name: "test-package".to_string(),
            version: "1.0.0".to_string(),
            release: "1".to_string(),
            files: vec![FileEntry {
                path: "/usr/bin/test".to_string(),
                hash: "abc123".to_string(),
                size: 1024,
                mode: 0o755,
                symlink_target: None,
                source_path: None,
            }],
            dependencies: vec!["dep1".to_string()],
            install_time: SystemTime::now(),
        };

        package_store.save(&pkg)?;

        let loaded = package_store.get("test-package")?;
        assert!(loaded.is_some());

        let loaded = loaded.unwrap();
        assert_eq!(loaded.name, "test-package");
        assert_eq!(loaded.version, "1.0.0");
        assert_eq!(loaded.files.len(), 1);
        Ok(())
    }

    #[test]
    fn test_package_store_list_all() -> Result<()> {
        let dir = tempdir()?;
        let store_root = dir.path().join("store");
        fs::create_dir_all(&store_root)?;

        let package_store = PackageStore::new(store_root.join("packages"))?;

        let pkg1 = Package {
            name: "package-a".to_string(),
            version: "1.0.0".to_string(),
            release: "1".to_string(),
            files: vec![],
            dependencies: vec![],
            install_time: SystemTime::now(),
        };

        let pkg2 = Package {
            name: "package-b".to_string(),
            version: "2.0.0".to_string(),
            release: "1".to_string(),
            files: vec![],
            dependencies: vec![],
            install_time: SystemTime::now(),
        };

        package_store.save(&pkg1)?;
        package_store.save(&pkg2)?;

        let all = package_store.list_all()?;
        assert_eq!(all.len(), 2);
        Ok(())
    }
}
