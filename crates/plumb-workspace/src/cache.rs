use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::Path;
use std::sync::Arc;

use fs2::FileExt;

const CACHE_NAMESPACE_LOCK: &str = ".active.lock";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheNamespaceState {
    Active,
    Inactive,
    Unmanaged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CacheNamespaceUsage {
    pub state: CacheNamespaceState,
    pub databases: usize,
    pub files: usize,
    pub bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CachePruneOutcome {
    Active,
    Unmanaged,
    Pruned { files: usize, bytes: u64 },
}

#[derive(Clone)]
pub(crate) struct CacheNamespaceLease {
    _file: Arc<File>,
}

impl CacheNamespaceLease {
    pub(crate) fn acquire(namespace: &Path) -> io::Result<Self> {
        fs::create_dir_all(namespace)?;
        let file = open_lock(namespace, true)?.expect("create requested");
        FileExt::lock_shared(&file)?;
        Ok(Self {
            _file: Arc::new(file),
        })
    }
}

pub fn inspect_cache_namespace(namespace: &Path) -> io::Result<CacheNamespaceUsage> {
    let usage = namespace_usage(namespace)?;
    let Some(lock) = open_lock(namespace, false)? else {
        return Ok(CacheNamespaceUsage {
            state: CacheNamespaceState::Unmanaged,
            ..usage
        });
    };
    let state = match lock.try_lock_exclusive() {
        Ok(()) => {
            FileExt::unlock(&lock)?;
            CacheNamespaceState::Inactive
        }
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => CacheNamespaceState::Active,
        Err(error) => return Err(error),
    };
    Ok(CacheNamespaceUsage { state, ..usage })
}

pub fn prune_cache_namespace(namespace: &Path) -> io::Result<CachePruneOutcome> {
    let Some(lock) = open_lock(namespace, false)? else {
        return Ok(CachePruneOutcome::Unmanaged);
    };
    match lock.try_lock_exclusive() {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
            return Ok(CachePruneOutcome::Active)
        }
        Err(error) => return Err(error),
    }
    let usage = namespace_usage(namespace)?;
    for entry in fs::read_dir(namespace)? {
        let entry = entry?;
        if entry.file_name() == CACHE_NAMESPACE_LOCK {
            continue;
        }
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            fs::remove_dir_all(entry.path())?;
        } else {
            fs::remove_file(entry.path())?;
        }
    }
    FileExt::unlock(&lock)?;
    Ok(CachePruneOutcome::Pruned {
        files: usage.files,
        bytes: usage.bytes,
    })
}

fn open_lock(namespace: &Path, create: bool) -> io::Result<Option<File>> {
    let path = namespace.join(CACHE_NAMESPACE_LOCK);
    match OpenOptions::new()
        .read(true)
        .write(true)
        .create(create)
        .open(path)
    {
        Ok(file) => Ok(Some(file)),
        Err(error) if !create && error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn namespace_usage(namespace: &Path) -> io::Result<CacheNamespaceUsage> {
    let mut usage = CacheNamespaceUsage {
        state: CacheNamespaceState::Unmanaged,
        databases: 0,
        files: 0,
        bytes: 0,
    };
    if !namespace.exists() {
        return Ok(usage);
    }
    collect_usage(namespace, &mut usage)?;
    Ok(usage)
}

fn collect_usage(path: &Path, usage: &mut CacheNamespaceUsage) -> io::Result<()> {
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        if entry.file_name() == CACHE_NAMESPACE_LOCK {
            continue;
        }
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            collect_usage(&entry.path(), usage)?;
            continue;
        }
        usage.files += 1;
        usage.bytes = usage.bytes.saturating_add(entry.metadata()?.len());
        if entry
            .path()
            .extension()
            .is_some_and(|extension| extension == "sqlite3")
        {
            usage.databases += 1;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_namespaces_are_skipped_and_inactive_data_is_pruned() {
        let directory = tempfile::tempdir().unwrap();
        let namespace = directory.path().join("0.1.0");
        let lease = CacheNamespaceLease::acquire(&namespace).unwrap();
        fs::write(namespace.join("semantic.sqlite3"), b"database").unwrap();
        fs::write(namespace.join("semantic.sqlite3-wal"), b"wal").unwrap();

        assert_eq!(
            inspect_cache_namespace(&namespace).unwrap(),
            CacheNamespaceUsage {
                state: CacheNamespaceState::Active,
                databases: 1,
                files: 2,
                bytes: 11,
            }
        );
        assert_eq!(
            prune_cache_namespace(&namespace).unwrap(),
            CachePruneOutcome::Active
        );

        drop(lease);
        assert_eq!(
            prune_cache_namespace(&namespace).unwrap(),
            CachePruneOutcome::Pruned {
                files: 2,
                bytes: 11,
            }
        );
        assert!(namespace.join(CACHE_NAMESPACE_LOCK).is_file());
        assert!(!namespace.join("semantic.sqlite3").exists());
    }

    #[test]
    fn namespaces_without_a_lease_file_are_unmanaged() {
        let directory = tempfile::tempdir().unwrap();
        let namespace = directory.path().join("legacy");
        fs::create_dir(&namespace).unwrap();
        fs::write(namespace.join("semantic.sqlite3"), b"legacy").unwrap();

        assert_eq!(
            inspect_cache_namespace(&namespace).unwrap().state,
            CacheNamespaceState::Unmanaged
        );
        assert_eq!(
            prune_cache_namespace(&namespace).unwrap(),
            CachePruneOutcome::Unmanaged
        );
        assert!(namespace.join("semantic.sqlite3").exists());
    }
}
