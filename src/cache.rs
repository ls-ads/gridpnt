//! A copy of the catalog on disk, so the tree appears at once on the next
//! start instead of waiting for the network. (CAT-003, PERF-001)
//!
//! Only names, types, and comments are stored: what the catalog tree
//! shows. Query results are never written to disk.
//!
//! The cache is a convenience. Any problem reading or writing it is
//! ignored and gridpnt carries on by asking the database.

use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::backend::CatalogNode;
use crate::config;

/// Separates the parts of a catalog path in a cache key. A control
/// character, so it cannot appear in a real name.
const PATH_SEPARATOR: &str = "\u{1f}";

#[derive(Default)]
pub struct CatalogCache {
    /// Where the cache is kept. `None` means nothing is saved.
    file: Option<PathBuf>,
    /// Children of each catalog path that has been listed.
    listings: BTreeMap<String, Vec<CatalogNode>>,
}

impl CatalogCache {
    /// Open the cache for the connection called `name`.
    pub fn for_connection(name: &str) -> Self {
        // Keep the file name tame whatever the connection is called.
        let safe: String = name
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        match config::cache_dir() {
            Some(dir) => Self::at(dir.join(format!("catalog-{safe}.json"))),
            None => Self::default(),
        }
    }

    /// Open the cache stored in `file`.
    pub fn at(file: PathBuf) -> Self {
        let listings = std::fs::read_to_string(&file)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        CatalogCache {
            file: Some(file),
            listings,
        }
    }

    pub fn get(&self, path: &[String]) -> Option<Vec<CatalogNode>> {
        self.listings.get(&path.join(PATH_SEPARATOR)).cloned()
    }

    pub fn put(&mut self, path: &[String], nodes: &[CatalogNode]) {
        self.listings.insert(path.join(PATH_SEPARATOR), nodes.to_vec());
        self.save();
    }

    fn save(&self) {
        let Some(file) = &self.file else { return };
        let Ok(text) = serde_json::to_string(&self.listings) else {
            return;
        };
        if let Some(dir) = file.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        // Write beside the real file and then rename over it, so a crash
        // part-way through cannot leave a half-written cache.
        let draft = file.with_extension("json.tmp");
        if std::fs::write(&draft, text).is_ok() {
            let _ = std::fs::rename(&draft, file);
        }
    }
}
