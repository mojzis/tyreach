//! Path-based internal vs external classification.
//!
//! A target path is `Internal` if, after canonicalization, it lives under the
//! canonicalized `repo_root`; otherwise `External`. Paths that fail to
//! canonicalize (nonexistent / permission denied) produce `Unresolved` with a
//! warning trace — callers treat these as dynamic/unresolvable call targets.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::model::Kind;

/// Classifier with a canonicalization cache to avoid re-stat'ing hot targets.
///
/// The cache maps the *input* path (as supplied to `classify`) to the
/// canonicalized form. The repo root is canonicalized once in `new`.
pub struct Classifier {
    repo_root: PathBuf,
    cache: HashMap<PathBuf, PathBuf>,
}

impl Classifier {
    /// Build a classifier anchored at `repo_root`. Fails if the root can't be
    /// canonicalized — this is a fatal configuration error.
    pub fn new(repo_root: &Path) -> Result<Self> {
        let repo_root = repo_root
            .canonicalize()
            .with_context(|| format!("canonicalize repo_root {}", repo_root.display()))?;
        Ok(Self { repo_root, cache: HashMap::new() })
    }

    /// Return the canonicalized repo root.
    pub fn repo_root(&self) -> &Path {
        &self.repo_root
    }

    /// Classify `target`. Canonicalization failures become `Unresolved`.
    pub fn classify(&mut self, target: &Path) -> Kind {
        let canonical = if let Some(hit) = self.cache.get(target) {
            hit.clone()
        } else {
            match target.canonicalize() {
                Ok(p) => {
                    self.cache.insert(target.to_path_buf(), p.clone());
                    p
                }
                Err(err) => {
                    tracing::warn!("classify: canonicalize failed for {}: {err}", target.display());
                    return Kind::Unresolved;
                }
            }
        };

        if canonical.starts_with(&self.repo_root)
            && !canonical.components().any(|c| c.as_os_str() == "site-packages")
        {
            Kind::Internal
        } else {
            Kind::External
        }
    }
}

/// One-shot variant used in tests / callers without a reusable classifier.
pub fn classify(target: &Path, repo_root: &Path) -> Result<Kind> {
    Ok(Classifier::new(repo_root)?.classify(target))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn internal_path_inside_root() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("pkg/mod.py");
        std::fs::create_dir_all(file.parent().expect("parent")).expect("mkdir");
        std::fs::write(&file, "x = 1\n").expect("write");

        let kind = classify(&file, dir.path()).expect("classify");
        assert_eq!(kind, Kind::Internal);
    }

    #[test]
    fn external_path_outside_root() {
        let root = tempfile::tempdir().expect("root");
        let other = tempfile::tempdir().expect("other");
        let file = other.path().join("mod.py");
        std::fs::write(&file, "x = 1\n").expect("write");

        let kind = classify(&file, root.path()).expect("classify");
        assert_eq!(kind, Kind::External);
    }

    #[test]
    fn missing_path_is_unresolved() {
        let root = tempfile::tempdir().expect("root");
        let missing = root.path().join("does_not_exist.py");

        let kind = classify(&missing, root.path()).expect("classify");
        assert_eq!(kind, Kind::Unresolved);
    }

    #[test]
    fn site_packages_under_root_is_external() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join(".venv/lib/python3.13/site-packages/pandas/core/frame.py");
        std::fs::create_dir_all(file.parent().expect("parent")).expect("mkdir");
        std::fs::write(&file, "x = 1\n").expect("write");

        let kind = classify(&file, dir.path()).expect("classify");
        assert_eq!(kind, Kind::External);
    }

    #[test]
    fn cache_reuses_previous_canonicalization() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("mod.py");
        std::fs::write(&file, "").expect("write");

        let mut classifier = Classifier::new(dir.path()).expect("new");
        assert_eq!(classifier.classify(&file), Kind::Internal);
        assert_eq!(classifier.classify(&file), Kind::Internal);
        assert!(classifier.cache.contains_key(&file), "cache must be populated");
    }
}
