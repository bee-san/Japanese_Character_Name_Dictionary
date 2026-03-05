use rusqlite::{params, Connection};
use sha2::{Digest, Sha256};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{info, warn};

/// 20 GB in bytes.
const MAX_CACHE_BYTES: u64 = 20 * 1024 * 1024 * 1024;

/// Evict bottom 35% by popularity when cache is full.
const EVICT_FRACTION: f64 = 0.35;

/// 6 months in seconds (180 days).
const MAX_AGE_SECS: i64 = 180 * 24 * 60 * 60;

/// On-disk image cache backed by SQLite with BLOB storage.
///
/// Image bytes are stored directly as BLOBs in the SQLite database alongside
/// metadata. SQLite tracks popularity (hit_count). Eviction removes the least
/// popular 35% of entries when total size exceeds 20 GB. Entries older than
/// 6 months are treated as expired on read and cleaned up.
#[derive(Clone)]
pub struct ImageCache {
    inner: Arc<ImageCacheInner>,
}

struct ImageCacheInner {
    /// SQLite connection (single-writer, serialized via Mutex).
    db: Mutex<Connection>,
    /// Running total of cached bytes (kept in sync with DB).
    total_bytes: AtomicU64,
}

impl ImageCache {
    /// Open (or create) the image cache at the given directory.
    ///
    /// Creates the cache directory and SQLite DB if they don't exist.
    /// Initializes the in-memory byte counter from the DB.
    pub fn open(cache_dir: &Path) -> Result<Self, String> {
        // Ensure cache directory exists (just the top-level dir, no images/ subdirectory)
        std::fs::create_dir_all(cache_dir)
            .map_err(|e| format!("Failed to create cache dir: {}", e))?;

        let db_path = cache_dir.join("image_cache.db");
        let conn =
            Connection::open(&db_path).map_err(|e| format!("Failed to open cache DB: {}", e))?;

        // WAL mode for concurrent reads + single writer
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")
            .map_err(|e| format!("Failed to set pragmas: {}", e))?;

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS images (
                url_hash    TEXT PRIMARY KEY,
                url         TEXT NOT NULL,
                data        BLOB NOT NULL,
                size_bytes  INTEGER NOT NULL,
                ext         TEXT NOT NULL,
                created_at  INTEGER NOT NULL,
                hit_count   INTEGER NOT NULL DEFAULT 0,
                last_hit_at INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_hit_count ON images(hit_count);",
        )
        .map_err(|e| format!("Failed to create table: {}", e))?;

        // Initialize running total from DB
        let total: u64 = conn
            .query_row(
                "SELECT COALESCE(SUM(size_bytes), 0) FROM images",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);

        // Log if old images/ directory exists (from pre-BLOB migration)
        let old_images_dir = cache_dir.join("images");
        if old_images_dir.exists() {
            info!("Old image cache directory found at images/, it can be safely deleted");
        }

        info!(
            total_mb = total / (1024 * 1024),
            path = %cache_dir.display(),
            "Image cache opened"
        );

        Ok(Self {
            inner: Arc::new(ImageCacheInner {
                db: Mutex::new(conn),
                total_bytes: AtomicU64::new(total),
            }),
        })
    }

    /// Look up a cached image by source URL.
    /// Returns (bytes, extension) on hit, None on miss.
    /// Increments hit_count on every access. Expires entries older than 6 months.
    pub async fn get(&self, url: &str) -> Option<(Vec<u8>, String)> {
        let hash = url_hash(url);
        let db = self.inner.db.lock().await;

        // Query returns: data (BLOB), ext, size_bytes, created_at
        let result: Option<(Vec<u8>, String, i64, i64)> = db
            .query_row(
                "SELECT data, ext, size_bytes, created_at FROM images WHERE url_hash = ?1",
                params![hash],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .ok();

        let (data, ext, size_bytes, created_at) = result?;

        // TTL check: expire entries older than 6 months
        let now = now_secs();
        if now - created_at > MAX_AGE_SECS {
            let _ = db.execute("DELETE FROM images WHERE url_hash = ?1", params![hash]);
            drop(db);
            saturating_sub(&self.inner.total_bytes, size_bytes as u64);
            return None;
        }

        // Bump popularity
        let _ = db.execute(
            "UPDATE images SET hit_count = hit_count + 1, last_hit_at = ?1 WHERE url_hash = ?2",
            params![now, hash],
        );

        Some((data, ext))
    }

    /// Store a processed image in the cache.
    /// Triggers background eviction if total size exceeds the limit.
    /// On re-insert of the same URL, preserves hit_count and last_hit_at,
    /// and correctly adjusts the byte counter for the size delta.
    pub async fn put(&self, url: &str, bytes: &[u8], ext: &str) {
        let hash = url_hash(url);
        let size = bytes.len() as u64;
        let now = now_secs();

        let db = self.inner.db.lock().await;

        // Check for existing entry to get old size for counter adjustment
        let old_size: Option<i64> = db
            .query_row(
                "SELECT size_bytes FROM images WHERE url_hash = ?1",
                params![hash],
                |row| row.get(0),
            )
            .ok();

        // Upsert: on conflict, update data/size/ext but preserve hit_count and last_hit_at
        let result = db.execute(
            "INSERT INTO images (url_hash, url, data, size_bytes, ext, created_at, hit_count, last_hit_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, ?6)
             ON CONFLICT(url_hash) DO UPDATE SET
                data = excluded.data,
                size_bytes = excluded.size_bytes,
                ext = excluded.ext,
                created_at = excluded.created_at",
            params![hash, url, bytes, size as i64, ext, now],
        );
        drop(db);

        if result.is_err() {
            warn!("Failed to insert cache entry");
            return;
        }

        // Adjust byte counter: subtract old size (if replacing), add new size
        if let Some(old) = old_size {
            let old = old as u64;
            if size > old {
                self.inner
                    .total_bytes
                    .fetch_add(size - old, Ordering::Relaxed);
            } else {
                saturating_sub(&self.inner.total_bytes, old - size);
            }
        } else {
            self.inner.total_bytes.fetch_add(size, Ordering::Relaxed);
        }

        let current_total = self.inner.total_bytes.load(Ordering::Relaxed);
        if current_total > MAX_CACHE_BYTES {
            let cache = self.clone();
            tokio::spawn(async move {
                cache.evict().await;
            });
        }
    }

    /// Evict the bottom 35% least popular entries.
    async fn evict(&self) {
        let db = self.inner.db.lock().await;

        let count: u64 = db
            .query_row("SELECT COUNT(*) FROM images", [], |row| row.get(0))
            .unwrap_or(0);

        if count == 0 {
            return;
        }

        let evict_count = ((count as f64) * EVICT_FRACTION).ceil() as u64;

        // Select the least popular entries (scope stmt so it's dropped before the DELETE)
        let entries: Vec<(String, u64)> = {
            let mut stmt = match db.prepare(
                "SELECT url_hash, size_bytes FROM images
                 ORDER BY hit_count ASC, last_hit_at ASC
                 LIMIT ?1",
            ) {
                Ok(s) => s,
                Err(e) => {
                    warn!(error = %e, "Failed to prepare eviction query");
                    return;
                }
            };

            stmt.query_map(params![evict_count], |row| {
                Ok((row.get(0)?, row.get::<_, i64>(1)? as u64))
            })
            .ok()
            .map(|rows| rows.filter_map(|r| r.ok()).collect())
            .unwrap_or_default()
        };

        if entries.is_empty() {
            return;
        }

        // Batch DELETE — this removes both metadata AND data (BLOBs) in one query
        let hashes: Vec<String> = entries.iter().map(|(h, _)| h.clone()).collect();
        let freed: u64 = entries.iter().map(|(_, s)| s).sum();

        let placeholders: Vec<String> = (1..=hashes.len()).map(|i| format!("?{}", i)).collect();
        let sql = format!(
            "DELETE FROM images WHERE url_hash IN ({})",
            placeholders.join(",")
        );
        let params: Vec<&dyn rusqlite::types::ToSql> = hashes
            .iter()
            .map(|h| h as &dyn rusqlite::types::ToSql)
            .collect();
        let _ = db.execute(&sql, params.as_slice());

        drop(db);

        saturating_sub(&self.inner.total_bytes, freed);

        info!(
            evicted = hashes.len(),
            freed_mb = freed / (1024 * 1024),
            "Image cache eviction complete"
        );
    }

    /// Current total cached bytes (from in-memory counter).
    #[cfg(test)]
    fn total_bytes(&self) -> u64 {
        self.inner.total_bytes.load(Ordering::Relaxed)
    }
}

/// Saturating subtract on an AtomicU64 — prevents underflow wrapping.
fn saturating_sub(atomic: &AtomicU64, val: u64) {
    let _ = atomic.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
        Some(current.saturating_sub(val))
    });
}

/// SHA-256 hash of a URL, returned as a 64-char hex string.
fn url_hash(url: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(url.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Current time as Unix seconds (wall-clock; may jump on NTP adjustments).
fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_url_hash_deterministic() {
        let h1 = url_hash("https://example.com/img.jpg");
        let h2 = url_hash("https://example.com/img.jpg");
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64);
    }

    #[test]
    fn test_url_hash_different_urls() {
        let h1 = url_hash("https://a.com/1.jpg");
        let h2 = url_hash("https://a.com/2.jpg");
        assert_ne!(h1, h2);
    }

    #[tokio::test]
    async fn test_put_and_get() {
        let dir = tempfile::tempdir().unwrap();
        let cache = ImageCache::open(dir.path()).unwrap();

        let url = "https://example.com/test.jpg";
        let bytes = vec![1, 2, 3, 4, 5];

        cache.put(url, &bytes, "webp").await;

        let result = cache.get(url).await;
        assert!(result.is_some());
        let (got_bytes, got_ext) = result.unwrap();
        assert_eq!(got_bytes, bytes);
        assert_eq!(got_ext, "webp");
    }

    #[tokio::test]
    async fn test_get_miss() {
        let dir = tempfile::tempdir().unwrap();
        let cache = ImageCache::open(dir.path()).unwrap();

        assert!(cache.get("https://nope.com/x.jpg").await.is_none());
    }

    #[tokio::test]
    async fn test_total_bytes_tracking() {
        let dir = tempfile::tempdir().unwrap();
        let cache = ImageCache::open(dir.path()).unwrap();

        assert_eq!(cache.total_bytes(), 0);

        cache
            .put("https://a.com/1.jpg", &vec![0u8; 100], "jpg")
            .await;
        assert_eq!(cache.total_bytes(), 100);

        cache
            .put("https://a.com/2.jpg", &vec![0u8; 200], "jpg")
            .await;
        assert_eq!(cache.total_bytes(), 300);
    }

    #[tokio::test]
    async fn test_hit_count_increments() {
        let dir = tempfile::tempdir().unwrap();
        let cache = ImageCache::open(dir.path()).unwrap();

        let url = "https://example.com/popular.jpg";
        cache.put(url, &[1, 2, 3], "jpg").await;

        // Access 3 times
        cache.get(url).await;
        cache.get(url).await;
        cache.get(url).await;

        // Check hit_count in DB
        let db = cache.inner.db.lock().await;
        let count: i64 = db
            .query_row(
                "SELECT hit_count FROM images WHERE url_hash = ?1",
                params![url_hash(url)],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 3);
    }

    #[tokio::test]
    async fn test_replace_preserves_hit_count() {
        let dir = tempfile::tempdir().unwrap();
        let cache = ImageCache::open(dir.path()).unwrap();

        let url = "https://example.com/img.jpg";
        cache.put(url, &[1, 2, 3], "jpg").await;

        // Build up some hits
        cache.get(url).await;
        cache.get(url).await;

        // Re-put with different data (simulating re-download)
        cache.put(url, &[4, 5, 6, 7], "webp").await;

        // hit_count should be preserved
        let db = cache.inner.db.lock().await;
        let count: i64 = db
            .query_row(
                "SELECT hit_count FROM images WHERE url_hash = ?1",
                params![url_hash(url)],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 2);
    }

    #[tokio::test]
    async fn test_replace_adjusts_total_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let cache = ImageCache::open(dir.path()).unwrap();

        let url = "https://example.com/img.jpg";
        cache.put(url, &vec![0u8; 100], "jpg").await;
        assert_eq!(cache.total_bytes(), 100);

        // Replace with larger data
        cache.put(url, &vec![0u8; 250], "jpg").await;
        assert_eq!(cache.total_bytes(), 250);

        // Replace with smaller data
        cache.put(url, &vec![0u8; 50], "jpg").await;
        assert_eq!(cache.total_bytes(), 50);
    }

    #[test]
    fn test_saturating_sub_no_underflow() {
        let a = AtomicU64::new(10);
        saturating_sub(&a, 20);
        assert_eq!(a.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn test_blob_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let cache = ImageCache::open(dir.path()).unwrap();

        // Create bytes that exercise edge cases: null bytes, high bytes, all 256 values
        let mut bytes: Vec<u8> = (0..=255).collect();
        // Add some typical JPEG-like data
        bytes.extend_from_slice(&[0xFF, 0xD8, 0xFF, 0xE0]); // JPEG header
        bytes.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]); // null bytes
        bytes.extend_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF]); // high bytes

        cache
            .put("https://example.com/test.jpg", &bytes, "jpg")
            .await;

        let result = cache.get("https://example.com/test.jpg").await;
        assert!(result.is_some());
        let (got_bytes, got_ext) = result.unwrap();

        // Every single byte must match exactly
        assert_eq!(got_bytes.len(), bytes.len());
        assert_eq!(got_bytes, bytes);
        assert_eq!(got_ext, "jpg");
    }

    #[tokio::test]
    async fn test_large_blob() {
        let dir = tempfile::tempdir().unwrap();
        let cache = ImageCache::open(dir.path()).unwrap();

        // 100KB of random-ish data (simulating a JPEG thumbnail)
        let bytes: Vec<u8> = (0..100_000).map(|i| (i % 256) as u8).collect();

        cache
            .put("https://example.com/big.jpg", &bytes, "jpg")
            .await;

        let result = cache.get("https://example.com/big.jpg").await;
        assert!(result.is_some());
        let (got_bytes, _) = result.unwrap();
        assert_eq!(got_bytes.len(), 100_000);
        assert_eq!(got_bytes, bytes);
    }

    // ===== Additional comprehensive tests =====

    #[tokio::test]
    async fn test_put_overwrite_different_extension() {
        let dir = tempfile::tempdir().unwrap();
        let cache = ImageCache::open(dir.path()).unwrap();

        let url = "https://example.com/img";
        cache.put(url, &[1, 2, 3], "jpg").await;
        cache.put(url, &[4, 5, 6], "webp").await;

        let (bytes, ext) = cache.get(url).await.unwrap();
        assert_eq!(bytes, vec![4, 5, 6]);
        assert_eq!(ext, "webp");
    }

    #[tokio::test]
    async fn test_multiple_urls_independent() {
        let dir = tempfile::tempdir().unwrap();
        let cache = ImageCache::open(dir.path()).unwrap();

        cache.put("https://a.com/1.jpg", &[1], "jpg").await;
        cache.put("https://b.com/2.png", &[2], "png").await;
        cache.put("https://c.com/3.webp", &[3], "webp").await;

        let (b1, e1) = cache.get("https://a.com/1.jpg").await.unwrap();
        let (b2, e2) = cache.get("https://b.com/2.png").await.unwrap();
        let (b3, e3) = cache.get("https://c.com/3.webp").await.unwrap();

        assert_eq!(b1, vec![1]);
        assert_eq!(e1, "jpg");
        assert_eq!(b2, vec![2]);
        assert_eq!(e2, "png");
        assert_eq!(b3, vec![3]);
        assert_eq!(e3, "webp");
    }

    #[tokio::test]
    async fn test_empty_bytes_stored_and_retrieved() {
        let dir = tempfile::tempdir().unwrap();
        let cache = ImageCache::open(dir.path()).unwrap();

        cache.put("https://example.com/empty", &[], "jpg").await;
        let result = cache.get("https://example.com/empty").await;
        assert!(result.is_some());
        let (bytes, _) = result.unwrap();
        assert!(bytes.is_empty());
    }

    #[tokio::test]
    async fn test_url_hash_is_deterministic() {
        let h1 = url_hash("https://example.com/test.jpg");
        let h2 = url_hash("https://example.com/test.jpg");
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64); // SHA-256 hex
    }

    #[tokio::test]
    async fn test_different_urls_different_hashes() {
        let h1 = url_hash("https://a.com/1");
        let h2 = url_hash("https://a.com/2");
        assert_ne!(h1, h2);
    }

    #[tokio::test]
    async fn test_cache_reopen_preserves_data() {
        let dir = tempfile::tempdir().unwrap();

        // Write data
        {
            let cache = ImageCache::open(dir.path()).unwrap();
            cache
                .put("https://example.com/persist.jpg", &[10, 20, 30], "jpg")
                .await;
        }

        // Reopen and verify
        {
            let cache = ImageCache::open(dir.path()).unwrap();
            let result = cache.get("https://example.com/persist.jpg").await;
            assert!(result.is_some());
            let (bytes, ext) = result.unwrap();
            assert_eq!(bytes, vec![10, 20, 30]);
            assert_eq!(ext, "jpg");
        }
    }

    #[tokio::test]
    async fn test_total_bytes_after_reopen() {
        let dir = tempfile::tempdir().unwrap();

        {
            let cache = ImageCache::open(dir.path()).unwrap();
            cache.put("https://a.com/1", &vec![0u8; 500], "jpg").await;
            cache.put("https://a.com/2", &vec![0u8; 300], "jpg").await;
            assert_eq!(cache.total_bytes(), 800);
        }

        {
            let cache = ImageCache::open(dir.path()).unwrap();
            assert_eq!(cache.total_bytes(), 800);
        }
    }
}
