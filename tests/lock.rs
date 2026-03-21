use rscm::lock::{LockDelta, LockedPackage, PackageChange};

#[test]
fn test_lock_delta_summary_added() {
    let delta = LockDelta {
        added: vec!["pkg1".to_string()],
        removed: vec![],
        changed: vec![],
    };
    assert_eq!(delta.summary(), "+1 added");
    assert!(!delta.is_empty());
}

#[test]
fn test_lock_delta_summary_removed() {
    let delta = LockDelta {
        added: vec![],
        removed: vec!["pkg1".to_string()],
        changed: vec![],
    };
    assert_eq!(delta.summary(), "-1 removed");
}

#[test]
fn test_lock_delta_summary_changed() {
    let delta = LockDelta {
        added: vec![],
        removed: vec![],
        changed: vec![PackageChange {
            name: "pkg1".to_string(),
            old_version: "1.0.0".to_string(),
            new_version: "2.0.0".to_string(),
        }],
    };
    assert_eq!(delta.summary(), "~1 changed");
}

#[test]
fn test_lock_delta_summary_no_changes() {
    let delta = LockDelta {
        added: vec![],
        removed: vec![],
        changed: vec![],
    };
    assert_eq!(delta.summary(), "No changes");
    assert!(delta.is_empty());
}

#[test]
fn test_lock_delta_summary_multiple() {
    let delta = LockDelta {
        added: vec!["pkg1".to_string(), "pkg2".to_string()],
        removed: vec!["pkg3".to_string()],
        changed: vec![
            PackageChange {
                name: "pkg4".to_string(),
                old_version: "1.0.0".to_string(),
                new_version: "2.0.0".to_string(),
            },
            PackageChange {
                name: "pkg5".to_string(),
                old_version: "1.0.0".to_string(),
                new_version: "2.0.0".to_string(),
            },
        ],
    };
    assert_eq!(delta.summary(), "+2 added, -1 removed, ~2 changed");
}

#[test]
fn test_locked_package_creation() {
    let locked = LockedPackage {
        name: "test".to_string(),
        version: "1.0.0".to_string(),
        release: "1".to_string(),
        source: "core".to_string(),
        hash: "sha256:abc123".to_string(),
        dependencies: vec!["dep1".to_string(), "dep2".to_string()],
    };

    assert_eq!(locked.name, "test");
    assert_eq!(locked.version, "1.0.0");
    assert_eq!(locked.dependencies.len(), 2);
}
