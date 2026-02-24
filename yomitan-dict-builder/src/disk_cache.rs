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
        let hash = url_hash(url);
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
        let hash = url_hash(url);

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

        // Write metadata
        let meta = CacheMeta {
            url: url.to_string(),
            ext: ext.to_string(),
            size: bytes.len() as u64,
            created_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        };
        let meta_path = self.dir.join(format!("{}.meta", hash));
        if let Ok(json) = serde_json::to_vec(&meta) {
            let _ = tokio::fs::write(&meta_path, &json).await;
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

/// Walk the cache directory and remove expired entries.
async fn cleanup_expired(dir: &PathBuf) {
    let mut entries = match tokio::fs::read_dir(dir).await {
        Ok(e) => e,
        Err(_) => return,
    };

    let mut removed = 0u64;
    let mut kept = 0u64;

    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };

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

    if removed > 0 {
        info!(removed = removed, kept = kept, "Disk cache cleanup complete");
    }
}

/// SHA-256 hex digest of a URL, used as the cache filename.
fn url_hash(url: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(url.as_bytes());
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_url_hash_deterministic() {
        let h1 = url_hash("https://example.com/image.jpg");
        let h2 = url_hash("https://example.com/image.jpg");
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64); // SHA-256 hex = 64 chars
    }

    #[test]
    fn test_url_hash_different_urls() {
        let h1 = url_hash("https://example.com/a.jpg");
        let h2 = url_hash("https://example.com/b.jpg");
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
}
