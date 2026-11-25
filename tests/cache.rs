//! The catalog remembered on disk.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use gridpnt::backend::{Backend, Cancel, CatalogNode, Limits, NodeKind, Result, ResultSet};
use gridpnt::cache::CatalogCache;
use gridpnt::worker::children;

/// A backend that counts how often its catalog is asked for.
struct Counting {
    asked: Arc<AtomicUsize>,
}

impl Backend for Counting {
    fn label(&self) -> String {
        "counting".into()
    }
    fn children(&mut self, _: &[String]) -> Result<Vec<CatalogNode>> {
        let asked = self.asked.fetch_add(1, Ordering::SeqCst) + 1;
        Ok(vec![CatalogNode {
            name: format!("table_v{asked}"),
            kind: NodeKind::Table,
            detail: String::new(),
            has_children: true,
        }])
    }
    fn preview_sql(&self, _: &[String]) -> Option<String> {
        None
    }
    fn query(&mut self, _: &str, _: &Limits) -> Result<ResultSet> {
        Err("not used".into())
    }
    fn canceller(&self) -> Cancel {
        Box::new(|| {})
    }
}

#[test]
fn listings_are_remembered_across_runs_until_refreshed() {
    let file = std::env::temp_dir().join(format!("gridpnt-test-cache-{}.json", std::process::id()));
    let _ = std::fs::remove_file(&file);
    let asked = Arc::new(AtomicUsize::new(0));
    let mut backend = Counting {
        asked: Arc::clone(&asked),
    };
    let path = vec!["main".to_string(), "sales".to_string()];

    // First run: the database is asked once, then the answer is reused.
    let mut cache = CatalogCache::at(file.clone());
    let first = children(&mut backend, Some(&mut cache), &path, false).unwrap();
    let again = children(&mut backend, Some(&mut cache), &path, false).unwrap();
    assert_eq!(first[0].name, "table_v1");
    assert_eq!(again, first);
    assert_eq!(asked.load(Ordering::SeqCst), 1);

    // Second run: read from disk, the database is not asked at all.
    let mut cache = CatalogCache::at(file.clone());
    assert_eq!(children(&mut backend, Some(&mut cache), &path, false).unwrap(), first);
    assert_eq!(asked.load(Ordering::SeqCst), 1);

    // A different path is not confused with this one.
    assert!(cache.get(&["main".to_string()]).is_none());

    // Refreshing asks again and remembers the new answer.
    let fresh = children(&mut backend, Some(&mut cache), &path, true).unwrap();
    assert_eq!(fresh[0].name, "table_v2");
    assert_eq!(CatalogCache::at(file.clone()).get(&path).unwrap(), fresh);

    std::fs::remove_file(&file).unwrap();
}

#[test]
fn a_damaged_cache_file_is_ignored() {
    let file = std::env::temp_dir().join(format!("gridpnt-test-cache-bad-{}.json", std::process::id()));
    std::fs::write(&file, "{ not json").unwrap();
    assert!(CatalogCache::at(file.clone()).get(&[]).is_none());
    std::fs::remove_file(&file).unwrap();
}
