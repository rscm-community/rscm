use anyhow::Result;
use rscm::config::{EnvironmentConfig, ServiceConfig, UserConfig};
use rscm::store::generation::GenerationManifest;
use rscm::store::*;
use std::fs;
use std::time::SystemTime;
use tempfile::tempdir;

#[test]
fn test_package_store() -> Result<()> {
    let dir = tempdir()?;
    let store = PackageStore::new(dir.path().to_path_buf())?;

    let pkg = Package {
        name: "test-pkg".to_string(),
        version: "1.0".to_string(),
        release: "1".to_string(),
        files: vec![],
        dependencies: vec![],
        install_time: SystemTime::now(),
        install_script: None,
    };

    store.save(&pkg)?;

    let loaded = store.get("test-pkg")?;
    assert!(loaded.is_some());

    Ok(())
}

#[test]
fn test_generation_store() -> Result<()> {
    let dir = tempdir()?;
    println!("{}", dir.path().to_str().unwrap());
    let store = GenerationStore::new(dir.path().to_path_buf())?;

    let files = vec![];

    let id = store.create(
        &[],
        &files,
        EnvironmentConfig::default(),
        None,
        &std::collections::HashMap::<String, ServiceConfig>::new(),
        &std::collections::HashMap::<String, UserConfig>::new(),
        |_hash, _target| Ok(()),
    )?;
    assert_eq!(id, 1);

    let generations = store.list()?;
    assert_eq!(generations.len(), 1);

    for _generation in generations {
        println!("{}: {}", _generation.id, _generation.path.to_str().unwrap());
    }

    Ok(())
}

#[test]
fn test_reference_counter() -> Result<()> {
    let dir = tempdir()?;
    let path = dir.path().join("refs.json");
    let mut counter = ReferenceCounter::new(path.clone())?;

    counter.add("hash1", RefKind::Content)?;
    counter.add("hash1", RefKind::Content)?;
    assert_eq!(counter.get_count("hash1"), 2);

    counter.remove("hash1")?;
    assert_eq!(counter.get_count("hash1"), 1);

    counter.remove("hash1")?;
    assert_eq!(counter.get_count("hash1"), 0);

    Ok(())
}

#[test]
fn test_content_store_remove() -> Result<()> {
    let dir = tempdir()?;
    let store = ContentStore::new(dir.path().to_path_buf())?;

    let test_file = dir.path().join("test.txt");
    fs::write(&test_file, "hello world")?;
    let hash = store.add_file(&test_file)?;

    let content_path = store.content_path(&hash);
    assert!(content_path.exists());

    store.remove(&hash)?;
    assert!(!content_path.exists());

    Ok(())
}

#[test]
fn test_gc_dry_run() -> Result<()> {
    let dir = tempdir()?;
    let store_root = dir.path().to_path_buf();

    fs::create_dir_all(&store_root)?;

    let content_dir = store_root.join("content");
    let packages_dir = store_root.join("packages");
    let generations_dir = store_root.join("generations");
    fs::create_dir_all(&content_dir)?;
    fs::create_dir_all(&packages_dir)?;
    fs::create_dir_all(&generations_dir)?;

    let test_file = dir.path().join("test_content.txt");
    fs::write(&test_file, "gc test content")?;

    let content_store = ContentStore::new(content_dir.clone())?;
    let hash = content_store.add_file(&test_file)?;

    let mut store = Store::new(store_root.clone())?;

    let result = store.gc(true)?;
    assert_eq!(result.collected_contents, 1);

    let content_path = content_store.content_path(&hash);
    assert!(content_path.exists());

    Ok(())
}

#[test]
fn test_gc_actual_run() -> Result<()> {
    let dir = tempdir()?;
    let store_root = dir.path().to_path_buf();

    fs::create_dir_all(&store_root)?;

    let content_dir = store_root.join("content");
    let packages_dir = store_root.join("packages");
    let generations_dir = store_root.join("generations");
    fs::create_dir_all(&content_dir)?;
    fs::create_dir_all(&packages_dir)?;
    fs::create_dir_all(&generations_dir)?;

    let test_file = dir.path().join("test_content2.txt");
    fs::write(&test_file, "gc test content 2")?;

    let content_store = ContentStore::new(content_dir.clone())?;
    let hash = content_store.add_file(&test_file)?;

    let mut store = Store::new(store_root.clone())?;

    let result = store.gc(false)?;
    assert_eq!(result.collected_contents, 1);

    let content_path = content_store.content_path(&hash);
    assert!(!content_path.exists());

    Ok(())
}

#[test]
fn test_gc_preserves_reachable_content() -> Result<()> {
    let dir = tempdir()?;
    let store_root = dir.path().to_path_buf();

    fs::create_dir_all(&store_root)?;

    let content_dir = store_root.join("content");
    let packages_dir = store_root.join("packages");
    let generations_dir = store_root.join("generations");
    fs::create_dir_all(&content_dir)?;
    fs::create_dir_all(&packages_dir)?;
    fs::create_dir_all(&generations_dir)?;

    let test_file = dir.path().join("reachable.txt");
    fs::write(&test_file, "reachable content")?;

    let content_store = ContentStore::new(content_dir.clone())?;
    let hash = content_store.add_file(&test_file)?;

    let pkg = Package {
        name: "testpkg".to_string(),
        version: "1.0".to_string(),
        release: "1".to_string(),
        files: vec![FileEntry {
            path: "/bin/test".to_string(),
            hash: hash.clone(),
            size: 100,
            mode: 0o755,
            symlink_target: None,
            source_path: Some(test_file.to_str().unwrap().to_string()),
        }],
        dependencies: vec![],
        install_time: SystemTime::now(),
        install_script: None,
    };

    let gen_dir = generations_dir.join("1");
    fs::create_dir_all(&gen_dir)?;
    let manifest = GenerationManifest {
        id: 1,
        packages: vec!["testpkg".to_string()],
        created: SystemTime::now(),
    };
    fs::write(
        gen_dir.join("manifest.toml"),
        toml::to_string_pretty(&manifest)?,
    )?;

    let pkg_dir = packages_dir.join("testpkg-1.0-1");
    fs::create_dir_all(&pkg_dir)?;
    fs::write(pkg_dir.join("manifest.toml"), toml::to_string_pretty(&pkg)?)?;

    let mut store = Store::new(store_root.clone())?;

    let result = store.gc(false)?;
    assert_eq!(result.collected_contents, 0);
    assert_eq!(result.collected_packages, 0);

    let content_path = content_store.content_path(&hash);
    assert!(content_path.exists());

    Ok(())
}
