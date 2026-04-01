use anyhow::Result;
use rscm::config::EnvironmentConfig;
use rscm::store::*;
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

    let id = store.create(&[], &files, EnvironmentConfig::default(),|_hash, _target| Ok(()))?;
    assert_eq!(id, 1);

    let generations = store.list()?;
    assert_eq!(generations.len(), 1);

    for _generation in generations {
        println!("{}: {}", _generation.id, _generation.path.to_str().unwrap());
    }

    Ok(())
}
