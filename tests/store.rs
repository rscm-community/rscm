use anyhow::Result;
use rscm::store::*;
use std::fs;
use std::time::SystemTime;
use tempfile::tempdir;

#[test]
fn test_content_store() -> Result<()> {
    let dir = tempdir()?;
    let store = ContentStore::new(dir.path().to_path_buf())?;

    let test_file = dir.path().join("test.txt");
    fs::write(&test_file, "hello world")?;

    let hash = store.add_file(&test_file)?;
    assert_eq!(
        hash,
        "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
    );

    assert!(store.verify(&hash)?);

    let link_target = dir.path().join("link.txt");
    store.link_to(&hash, &link_target)?;
    assert!(link_target.exists());

    Ok(())
}

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

    let id = store.create(&[], &files, |_hash, _target| Ok(()))?;
    assert_eq!(id, 1);

    let generations = store.list()?;
    assert_eq!(generations.len(), 1);

    for _generation in generations {
        println!("{}: {}", _generation.id, _generation.path.to_str().unwrap());
    }

    Ok(())
}
