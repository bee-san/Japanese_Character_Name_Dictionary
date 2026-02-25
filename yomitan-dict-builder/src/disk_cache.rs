use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tracing::{debug, info, warn};

/// 30-day TTL for cached images on disk.
const DISK_TTL_SECS: u64 = 30 * 24 * 3600;

/// Cleanup interval: run every 6 hours.
const CLEANUP_INTERVAL_SECS: u64 = 6 * 3600;

/// Metadata stored alongside each cached image file.
#[derive(Serialize, Deserialize)]
struct CacheMeta {
    url: String,
    ext: String,
    size: u64,
    created_at: u64, // unix timestamp
}

impl CacheMeta {
    fn is_expired(&self) -> bool {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        now.saturating_sub(self.created_at) > DISK_TTL_SECS
    }
}

/// Disk-backed image cache. Stores resized images as files keyed by SHA-256
/// of the source URL. Survives process restarts.
///
/// Layout:
/// ```text
/// <cache_dir>/
///   <sha256_hex>.img    # raw image bytes
///   <sha256_hex>.meta   # JSON metadata (url, ext, size, created_at)
/// ```
#[derive(Clone)]
pub struct DiskImageCache {
    dir: PathBuf,
}

impl DiskImageCache {
    /// Create a new disk cache at the given directory.
    /// Creates the directory if it doesn't exist.
    pub async fn new(dir: PathBuf) -> Self {
        if let Err(e) = tokio::fs::create_dir_all(&dir).await {
            warn!(path = %dir.display(), error = %e, "Failed to create disk cache directory");
        } else {
            info!(path = %dir.display(), "Disk image cache initialized");
        }
        Self { dir }
    }

    /// Look up a cached image by its source URL.
    /// Returns `Some((bytes, extension))` on hit, `None` on miss or expiry.
    pub async fn get(&self, url: &str) -> Option<(Vec<u8>, String)> {
        let hash = cache_hash(url);
        let meta_path = self.dir.join(format!("{}.meta", hash));

        let meta_bytes = tokio::fs::read(&meta_path).await.ok()?;
        let meta: CacheMeta = serde_json::from_slice(&meta_bytes).ok()?;

        if meta.is_expired() {
            // Expired — clean up in background, return miss
            let img_path = self.dir.join(format!("{}.img", hash));
            let mp = meta_path.clone();
            tokio::spawn(async move {
                let _ = tokio::fs::remove_file(&mp).await;
                let _ = tokio::fs::remove_file(&img_path).await;
            });
            return None;
        }

        let img_path = self.dir.join(format!("{}.img", hash));
        let bytes = tokio::fs::read(&img_path).await.ok()?;

        debug!(url = url, size = bytes.len(), "Disk cache hit");
        Some((bytes, meta.ext))
    }

    /// Store a resized image on disk. Writes atomically via tmp+rename.
    pub async fn put(&self, url: &str, bytes: &[u8], ext: &str) {
        let hash = cache_hash(url);

        // Write image bytes atomically
        let tmp_img = self.dir.join(format!("{}.img.tmp", hash));
        let final_img = self.dir.join(format!("{}.img", hash));
        if tokio::fs::write(&tmp_img, bytes).await.is_err() {
            return;
        }
        if tokio::fs::rename(&tmp_img, &final_img).await.is_err() {
            let _ = tokio::fs::remove_file(&tmp_img).await;
            return;
        }

        // Write metadata atomically via tmp+rename
        let meta = CacheMeta {
            url: url.to_string(),
            ext: ext.to_string(),
            size: bytes.len() as u64,
            created_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        };
        if let Ok(json) = serde_json::to_vec(&meta) {
            let tmp_meta = self.dir.join(format!("{}.meta.tmp", hash));
            let final_meta = self.dir.join(format!("{}.meta", hash));
            if tokio::fs::write(&tmp_meta, &json).await.is_ok()
                && tokio::fs::rename(&tmp_meta, &final_meta).await.is_err()
            {
                    let _ = tokio::fs::remove_file(&tmp_meta).await;
            }
        }
    }

    /// Spawn a background task that periodically removes expired entries.
    pub fn spawn_cleanup_task(&self) {
        let dir = self.dir.clone();
        tokio::spawn(async move {
            let interval = Duration::from_secs(CLEANUP_INTERVAL_SECS);
            loop {
                tokio::time::sleep(interval).await;
                cleanup_expired(&dir).await;
            }
        });
    }
}

// ============================================================================
// Generic disk-backed data cache (for ZIPs, API responses, etc.)
// ============================================================================

/// Metadata for generic cached data entries.
#[derive(Serialize, Deserialize)]
struct DataCacheMeta {
    key: String,
    size: u64,
    created_at: u64,
    ttl_secs: u64,
}

impl DataCacheMeta {
    fn is_expired(&self) -> bool {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        now.saturating_sub(self.created_at) > self.ttl_secs
    }
}

/// Generic disk-backed cache for arbitrary byte data.
/// Used for single-media ZIP files (14-day TTL) and API character data (21-day TTL).
///
/// Layout:
/// ```text
/// <cache_dir>/
///   <sha256_hex>.dat    # raw data bytes
///   <sha256_hex>.meta   # JSON metadata (key, size, created_at, ttl_secs)
/// ```
#[derive(Clone)]
pub struct DiskDataCache {
    dir: PathBuf,
    ttl_secs: u64,
}

impl DiskDataCache {
    /// Create a new data cache at the given directory with the specified TTL.
    pub async fn new(dir: PathBuf, ttl_secs: u64) -> Self {
        if let Err(e) = tokio::fs::create_dir_all(&dir).await {
            warn!(path = %dir.display(), error = %e, "Failed to create disk data cache directory");
        } else {
            info!(path = %dir.display(), ttl_days = ttl_secs / 86400, "Disk data cache initialized");
        }
        Self { dir, ttl_secs }
    }

    /// Look up cached data by key. Returns `Some(bytes)` on hit, `None` on miss/expiry.
    pub async fn get(&self, key: &str) -> Option<Vec<u8>> {
        let hash = cache_hash(key);
        let meta_path = self.dir.join(format!("{}.meta", hash));

        let meta_bytes = tokio::fs::read(&meta_path).await.ok()?;
        let meta: DataCacheMeta = serde_json::from_slice(&meta_bytes).ok()?;

        if meta.is_expired() {
            let dat_path = self.dir.join(format!("{}.dat", hash));
            let mp = meta_path.clone();
            tokio::spawn(async move {
                let _ = tokio::fs::remove_file(&mp).await;
                let _ = tokio::fs::remove_file(&dat_path).await;
            });
            return None;
        }

        let dat_path = self.dir.join(format!("{}.dat", hash));
        let bytes = tokio::fs::read(&dat_path).await.ok()?;

        debug!(key = key, size = bytes.len(), "Disk data cache hit");
        Some(bytes)
    }

    /// Store data on disk. Writes atomically via tmp+rename.
    pub async fn put(&self, key: &str, bytes: &[u8]) {
        let hash = cache_hash(key);

        let tmp_dat = self.dir.join(format!("{}.dat.tmp", hash));
        let final_dat = self.dir.join(format!("{}.dat", hash));
        if tokio::fs::write(&tmp_dat, bytes).await.is_err() {
            return;
        }
        if tokio::fs::rename(&tmp_dat, &final_dat).await.is_err() {
            let _ = tokio::fs::remove_file(&tmp_dat).await;
            return;
        }

        let meta = DataCacheMeta {
            key: key.to_string(),
            size: bytes.len() as u64,
            created_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            ttl_secs: self.ttl_secs,
        };
        if let Ok(json) = serde_json::to_vec(&meta) {
            let tmp_meta = self.dir.join(format!("{}.meta.tmp", hash));
            let final_meta = self.dir.join(format!("{}.meta", hash));
            if tokio::fs::write(&tmp_meta, &json).await.is_ok()
                && tokio::fs::rename(&tmp_meta, &final_meta).await.is_err()
            {
                let _ = tokio::fs::remove_file(&tmp_meta).await;
            }
        }
    }

    /// Spawn a background cleanup task for this cache directory.
    pub fn spawn_cleanup_task(&self) {
        let dir = self.dir.clone();
        tokio::spawn(async move {
            let interval = Duration::from_secs(CLEANUP_INTERVAL_SECS);
            loop {
                tokio::time::sleep(interval).await;
                cleanup_data_cache(&dir).await;
            }
        });
    }
}

/// Walk a data cache directory and remove expired entries.
async fn cleanup_data_cache(dir: &std::path::Path) {
    let mut entries = match tokio::fs::read_dir(dir).await {
        Ok(e) => e,
        Err(_) => return,
    };

    let mut removed = 0u64;
    let mut kept = 0u64;
    let mut orphaned_dats: Vec<PathBuf> = Vec::new();

    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };

        if name.ends_with(".tmp") {
            let _ = tokio::fs::remove_file(&path).await;
            continue;
        }

        if name.ends_with(".dat") {
            orphaned_dats.push(path);
            continue;
        }

        if !name.ends_with(".meta") {
            continue;
        }

        let meta_bytes = match tokio::fs::read(&path).await {
            Ok(b) => b,
            Err(_) => continue,
        };

        let meta: DataCacheMeta = match serde_json::from_slice(&meta_bytes) {
            Ok(m) => m,
            Err(_) => {
                let _ = tokio::fs::remove_file(&path).await;
                let hash = name.trim_end_matches(".meta");
                let _ = tokio::fs::remove_file(dir.join(format!("{}.dat", hash))).await;
                removed += 1;
                continue;
            }
        };

        if meta.is_expired() {
            let _ = tokio::fs::remove_file(&path).await;
            let hash = name.trim_end_matches(".meta");
            let _ = tokio::fs::remove_file(dir.join(format!("{}.dat", hash))).await;
            removed += 1;
        } else {
            kept += 1;
        }
    }

    for dat_path in orphaned_dats {
        if let Some(name) = dat_path.file_name().and_then(|n| n.to_str()) {
            let hash = name.trim_end_matches(".dat");
            let meta_path = dir.join(format!("{}.meta", hash));
            if !meta_path.exists() {
                let _ = tokio::fs::remove_file(&dat_path).await;
                removed += 1;
            }
        }
    }

    if removed > 0 {
        info!(removed = removed, kept = kept, dir = %dir.display(), "Data cache cleanup complete");
    }
}

/// Walk the cache directory and remove expired entries and orphaned files.
async fn cleanup_expired(dir: &std::path::Path) {
    let mut entries = match tokio::fs::read_dir(dir).await {
        Ok(e) => e,
        Err(_) => return,
    };

    let mut removed = 0u64;
    let mut kept = 0u64;
    let mut orphaned_imgs: Vec<PathBuf> = Vec::new();

    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };

        // Skip tmp files (leftover from interrupted writes)
        if name.ends_with(".tmp") {
            let _ = tokio::fs::remove_file(&path).await;
            continue;
        }

        // Collect .img files to check for orphans after the meta pass
        if name.ends_with(".img") {
            orphaned_imgs.push(path);
            continue;
        }

        // Only inspect .meta files — if meta is expired, remove both .meta and .img
        if !name.ends_with(".meta") {
            continue;
        }

        let meta_bytes = match tokio::fs::read(&path).await {
            Ok(b) => b,
            Err(_) => continue,
        };

        let meta: CacheMeta = match serde_json::from_slice(&meta_bytes) {
            Ok(m) => m,
            Err(_) => {
                // Corrupt meta — remove it and its companion
                let _ = tokio::fs::remove_file(&path).await;
                let hash = name.trim_end_matches(".meta");
                let _ = tokio::fs::remove_file(dir.join(format!("{}.img", hash))).await;
                removed += 1;
                continue;
            }
        };

        if meta.is_expired() {
            let _ = tokio::fs::remove_file(&path).await;
            let hash = name.trim_end_matches(".meta");
            let _ = tokio::fs::remove_file(dir.join(format!("{}.img", hash))).await;
            removed += 1;
        } else {
            kept += 1;
        }
    }

    // Remove orphaned .img files that have no corresponding .meta
    for img_path in orphaned_imgs {
        if let Some(name) = img_path.file_name().and_then(|n| n.to_str()) {
            let hash = name.trim_end_matches(".img");
            let meta_path = dir.join(format!("{}.meta", hash));
            if !meta_path.exists() {
                let _ = tokio::fs::remove_file(&img_path).await;
                removed += 1;
            }
        }
    }

    if removed > 0 {
        info!(
            removed = removed,
            kept = kept,
            "Disk cache cleanup complete"
        );
    }
}

/// SHA-256 hex digest of a string, used as the cache filename.
fn cache_hash(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cache_hash_deterministic() {
        let h1 = cache_hash("https://example.com/image.jpg");
        let h2 = cache_hash("https://example.com/image.jpg");
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64); // SHA-256 hex = 64 chars
    }

    #[test]
    fn test_cache_hash_different_inputs() {
        let h1 = cache_hash("https://example.com/a.jpg");
        let h2 = cache_hash("https://example.com/b.jpg");
        assert_ne!(h1, h2);
    }

    #[test]
    fn test_cache_meta_not_expired_fresh() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let meta = CacheMeta {
            url: "https://example.com/img.jpg".to_string(),
            ext: "webp".to_string(),
            size: 1024,
            created_at: now,
        };
        assert!(!meta.is_expired());
    }

    #[test]
    fn test_cache_meta_expired_old() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let meta = CacheMeta {
            url: "https://example.com/img.jpg".to_string(),
            ext: "webp".to_string(),
            size: 1024,
            created_at: now - DISK_TTL_SECS - 1,
        };
        assert!(meta.is_expired());
    }

    #[test]
    fn test_cache_meta_not_expired_boundary() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let meta = CacheMeta {
            url: "https://example.com/img.jpg".to_string(),
            ext: "webp".to_string(),
            size: 1024,
            created_at: now - DISK_TTL_SECS + 10, // 10 seconds before expiry
        };
        assert!(!meta.is_expired());
    }

    #[tokio::test]
    async fn test_disk_cache_roundtrip() {
        let dir = std::env::temp_dir().join(format!("yomitan_test_{}", uuid::Uuid::new_v4()));
        let cache = DiskImageCache::new(dir.clone()).await;

        let url = "https://example.com/test_image.jpg";
        let bytes = vec![0xFF, 0xD8, 0xFF, 0xE0, 1, 2, 3, 4];
        let ext = "webp";

        // Miss before put
        assert!(cache.get(url).await.is_none());

        // Put then hit
        cache.put(url, &bytes, ext).await;
        let result = cache.get(url).await;
        assert!(result.is_some());
        let (cached_bytes, cached_ext) = result.unwrap();
        assert_eq!(cached_bytes, bytes);
        assert_eq!(cached_ext, ext);

        // Different URL is a miss
        assert!(cache.get("https://example.com/other.jpg").await.is_none());

        // Cleanup
        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn test_disk_cache_overwrite() {
        let dir = std::env::temp_dir().join(format!("yomitan_test_{}", uuid::Uuid::new_v4()));
        let cache = DiskImageCache::new(dir.clone()).await;

        let url = "https://example.com/img.jpg";
        cache.put(url, &[1, 2, 3], "jpg").await;
        cache.put(url, &[4, 5, 6], "webp").await;

        let (bytes, ext) = cache.get(url).await.unwrap();
        assert_eq!(bytes, vec![4, 5, 6]);
        assert_eq!(ext, "webp");

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    // === Edge case: CacheMeta with timestamp 0 ===

    #[test]
    fn test_cache_meta_timestamp_zero_is_expired() {
        let meta = CacheMeta {
            url: "https://example.com/img.jpg".to_string(),
            ext: "webp".to_string(),
            size: 1024,
            created_at: 0,
        };
        assert!(meta.is_expired(), "Timestamp 0 should be expired");
    }

    // === Edge case: CacheMeta with future timestamp ===

    #[test]
    fn test_cache_meta_future_timestamp_not_expired() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let meta = CacheMeta {
            url: "https://example.com/img.jpg".to_string(),
            ext: "webp".to_string(),
            size: 1024,
            created_at: now + 3600, // 1 hour in the future
        };
        // saturating_sub: now - future = 0, which is < TTL
        assert!(!meta.is_expired(), "Future timestamp should not be expired");
    }

    // === Edge case: empty URL hash ===

    #[test]
    fn test_cache_hash_empty_string() {
        let h = cache_hash("");
        assert_eq!(h.len(), 64);
        // Should still produce a valid hash
        assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
    }

    // === Edge case: cache get on non-existent directory ===

    #[tokio::test]
    async fn test_disk_cache_get_nonexistent_url() {
        let dir = std::env::temp_dir().join(format!("yomitan_test_{}", uuid::Uuid::new_v4()));
        let cache = DiskImageCache::new(dir.clone()).await;

        // Get on a URL that was never put
        assert!(cache
            .get("https://never-stored.com/img.jpg")
            .await
            .is_none());

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    // === Edge case: put with empty bytes ===

    #[tokio::test]
    async fn test_disk_cache_put_empty_bytes() {
        let dir = std::env::temp_dir().join(format!("yomitan_test_{}", uuid::Uuid::new_v4()));
        let cache = DiskImageCache::new(dir.clone()).await;

        let url = "https://example.com/empty.jpg";
        cache.put(url, &[], "jpg").await;

        let result = cache.get(url).await;
        assert!(result.is_some());
        let (bytes, ext) = result.unwrap();
        assert!(bytes.is_empty());
        assert_eq!(ext, "jpg");

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    // === Edge case: put with empty extension ===

    #[tokio::test]
    async fn test_disk_cache_put_empty_extension() {
        let dir = std::env::temp_dir().join(format!("yomitan_test_{}", uuid::Uuid::new_v4()));
        let cache = DiskImageCache::new(dir.clone()).await;

        let url = "https://example.com/noext";
        cache.put(url, &[1, 2, 3], "").await;

        let result = cache.get(url).await;
        assert!(result.is_some());
        let (_, ext) = result.unwrap();
        assert_eq!(ext, "");

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    // === DiskDataCache tests ===

    #[tokio::test]
    async fn test_data_cache_roundtrip() {
        let dir = std::env::temp_dir().join(format!("yomitan_data_test_{}", uuid::Uuid::new_v4()));
        let cache = DiskDataCache::new(dir.clone(), 3600).await;

        let key = "vndb:v17:spoiler0:hon_true";
        let data = b"some zip bytes here";

        assert!(cache.get(key).await.is_none());

        cache.put(key, data).await;
        let result = cache.get(key).await;
        assert!(result.is_some());
        assert_eq!(result.unwrap(), data);

        assert!(cache.get("other_key").await.is_none());

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn test_data_cache_overwrite() {
        let dir = std::env::temp_dir().join(format!("yomitan_data_test_{}", uuid::Uuid::new_v4()));
        let cache = DiskDataCache::new(dir.clone(), 3600).await;

        let key = "test_key";
        cache.put(key, b"old data").await;
        cache.put(key, b"new data").await;

        let result = cache.get(key).await.unwrap();
        assert_eq!(result, b"new data");

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[test]
    fn test_data_cache_meta_expired() {
        let meta = DataCacheMeta {
            key: "test".to_string(),
            size: 100,
            created_at: 0,
            ttl_secs: 3600,
        };
        assert!(meta.is_expired());
    }

    #[test]
    fn test_data_cache_meta_not_expired() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let meta = DataCacheMeta {
            key: "test".to_string(),
            size: 100,
            created_at: now,
            ttl_secs: 3600,
        };
        assert!(!meta.is_expired());
    }

    #[test]
    fn test_cache_hash_deterministic_keys() {
        let h1 = cache_hash("vndb:v17");
        let h2 = cache_hash("vndb:v17");
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64);
    }

    #[test]
    fn test_cache_hash_different_keys() {
        let h1 = cache_hash("vndb:v17");
        let h2 = cache_hash("anilist:9253");
        assert_ne!(h1, h2);
    }
}
