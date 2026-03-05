use rusqlite::{params, Connection};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use tracing::{info, warn};

use crate::models::CharacterData;

/// 1 GB in bytes — generous for text-only data (~200KB/entry = ~5,000 entries).
const MAX_CACHE_BYTES: u64 = 1024 * 1024 * 1024;

/// Evict bottom 35% by popularity when cache is full.
const EVICT_FRACTION: f64 = 0.35;

/// 30 days in seconds — character data changes infrequently.
const MAX_AGE_SECS: i64 = 30 * 24 * 60 * 60;

/// Cached media entry returned by `get()`.
pub struct CacheEntry {
    pub title: String,
    pub char_data: CharacterData,
}

/// Cache for per-media API responses (character data + title).
///
/// Stores serialized `CharacterData` as SQLite BLOBs. Image bytes are NOT
/// cached here — they remain `None` in the stored data and are populated
/// by `download_images_concurrent()` via the existing `ImageCache`.
#[derive(Clone)]
pub struct MediaCache {
    inner: Arc<MediaCacheInner>,
}

struct MediaCacheInner {
    /// SQLite connection (single-writer, serialized via std::sync::Mutex).
    db: std::sync::Mutex<Connection>,
    /// Running total of cached bytes (kept in sync with DB).
    total_bytes: AtomicU64,
    /// Prevents concurrent eviction tasks from being spawned.
    evicting: AtomicBool,
}

impl MediaCache {
    /// Open (or create) the media cache database at `cache_dir/media_cache.db`.
    ///
    /// Creates the directory if it doesn't exist. Initializes the in-memory
    /// byte counter from the DB.
    pub fn open(cache_dir: &Path) -> Result<Self, String> {
        std::fs::create_dir_all(cache_dir)
            .map_err(|e| format!("Failed to create media cache dir: {}", e))?;

        let db_path = cache_dir.join("media_cache.db");
        let conn = Connection::open(&db_path)
            .map_err(|e| format!("Failed to open media cache DB: {}", e))?;

        // Enable WAL mode for better concurrent read performance.
        conn.execute_batch("PRAGMA journal_mode=WAL;")
            .map_err(|e| format!("Failed to set WAL mode: {}", e))?;

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS media (
                cache_key   TEXT PRIMARY KEY,
                title       TEXT NOT NULL,
                data        BLOB NOT NULL,
                size_bytes  INTEGER NOT NULL,
                created_at  INTEGER NOT NULL,
                hit_count   INTEGER NOT NULL DEFAULT 0,
                last_hit_at INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_media_hit_count ON media(hit_count);",
        )
        .map_err(|e| format!("Failed to create media cache table: {}", e))?;

        // Initialize byte counter from DB.
        let total: u64 = conn
            .query_row(
                "SELECT COALESCE(SUM(size_bytes), 0) FROM media",
                [],
                |row| row.get(0),
            )
            .map_err(|e| format!("Failed to query total bytes: {}", e))?;

        let total_mb = total as f64 / (1024.0 * 1024.0);
        info!(
            total_mb = format!("{:.1}", total_mb),
            path = %db_path.display(),
            "Media cache opened"
        );

        Ok(Self {
            inner: Arc::new(MediaCacheInner {
                db: std::sync::Mutex::new(conn),
                total_bytes: AtomicU64::new(total),
                evicting: AtomicBool::new(false),
            }),
        })
    }

    /// Look up a cached media entry by key.
    ///
    /// Returns `None` on miss, TTL expiry, or corrupt data. Automatically
    /// cleans up expired/corrupt entries.
    pub fn get(&self, cache_key: &str) -> Option<CacheEntry> {
        let db = self.inner.db.lock().unwrap();
        let now = epoch_secs();

        let row: Option<(String, Vec<u8>, i64, i64)> = db
            .query_row(
                "SELECT title, data, size_bytes, created_at FROM media WHERE cache_key = ?1",
                params![cache_key],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Vec<u8>>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, i64>(3)?,
                    ))
                },
            )
            .ok();

        let (title, data, size_bytes, created_at) = match row {
            Some(r) => r,
            None => {
                info!(key = cache_key, "Media cache miss");
                return None;
            }
        };

        // Check TTL.
        let age = now - created_at;
        if age > MAX_AGE_SECS {
            let age_days = age / 86400;
            info!(key = cache_key, age_days, "Media cache entry expired");
            let _ = db.execute("DELETE FROM media WHERE cache_key = ?1", params![cache_key]);
            self.inner
                .total_bytes
                .fetch_sub(size_bytes as u64, Ordering::Relaxed);
            return None;
        }

        // Deserialize.
        let char_data: CharacterData = match serde_json::from_slice(&data) {
            Ok(d) => d,
            Err(e) => {
                warn!(key = cache_key, error = %e, "Corrupt cache entry, removing");
                let _ = db.execute("DELETE FROM media WHERE cache_key = ?1", params![cache_key]);
                self.inner
                    .total_bytes
                    .fetch_sub(size_bytes as u64, Ordering::Relaxed);
                return None;
            }
        };

        // Bump hit count.
        let _ = db.execute(
            "UPDATE media SET hit_count = hit_count + 1, last_hit_at = ?1 WHERE cache_key = ?2",
            params![now, cache_key],
        );

        info!(key = cache_key, size_bytes = size_bytes, "Media cache hit");

        Some(CacheEntry { title, char_data })
    }

    /// Store a media entry in the cache.
    ///
    /// `char_data` should have `image_bytes` and `image_ext` set to `None`
    /// on all characters — images are handled by `ImageCache` separately.
    ///
    /// Uses UPSERT to preserve `hit_count` on re-insert.
    pub fn put(&self, cache_key: &str, title: &str, char_data: &CharacterData) {
        let data = match serde_json::to_vec(char_data) {
            Ok(d) => d,
            Err(e) => {
                warn!(key = cache_key, error = %e, "Failed to serialize cache entry");
                return;
            }
        };

        let size_bytes = data.len() as i64;
        let now = epoch_secs();

        let db = self.inner.db.lock().unwrap();

        // Get old size if replacing (for accurate total_bytes tracking).
        let old_size: i64 = db
            .query_row(
                "SELECT size_bytes FROM media WHERE cache_key = ?1",
                params![cache_key],
                |row| row.get(0),
            )
            .unwrap_or(0);

        // UPSERT: insert or update, preserving hit_count on conflict.
        let result = db.execute(
            "INSERT INTO media (cache_key, title, data, size_bytes, created_at, hit_count, last_hit_at)
             VALUES (?1, ?2, ?3, ?4, ?5, 0, ?5)
             ON CONFLICT(cache_key) DO UPDATE SET
                 title = excluded.title,
                 data = excluded.data,
                 size_bytes = excluded.size_bytes,
                 created_at = excluded.created_at,
                 last_hit_at = excluded.last_hit_at",
            params![cache_key, title, data, size_bytes, now],
        );

        if let Err(e) = result {
            warn!(key = cache_key, error = %e, "Failed to write cache entry");
            return;
        }

        // Update total_bytes: subtract old, add new.
        if old_size > 0 {
            self.inner
                .total_bytes
                .fetch_sub(old_size as u64, Ordering::Relaxed);
        }
        self.inner
            .total_bytes
            .fetch_add(size_bytes as u64, Ordering::Relaxed);

        info!(
            key = cache_key,
            size_bytes = size_bytes,
            "Media cache stored"
        );

        // Check if eviction is needed.
        let total = self.inner.total_bytes.load(Ordering::Relaxed);
        if total > MAX_CACHE_BYTES {
            self.maybe_evict();
        }
    }

    /// Returns the current total cached bytes.
    #[cfg(test)]
    pub fn total_bytes(&self) -> u64 {
        self.inner.total_bytes.load(Ordering::Relaxed)
    }

    /// Trigger eviction if not already running.
    fn maybe_evict(&self) {
        // Only one eviction at a time.
        if self
            .inner
            .evicting
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return;
        }

        let cache = self.clone();
        tokio::spawn(async move {
            tokio::task::spawn_blocking(move || {
                cache.evict();
                cache.inner.evicting.store(false, Ordering::SeqCst);
            })
            .await
            .ok();
        });
    }

    /// Evict the bottom 35% of entries by hit_count (then last_hit_at).
    fn evict(&self) {
        let db = self.inner.db.lock().unwrap();

        let total_count: i64 = db
            .query_row("SELECT COUNT(*) FROM media", [], |row| row.get(0))
            .unwrap_or(0);

        if total_count == 0 {
            return;
        }

        let evict_count = ((total_count as f64) * EVICT_FRACTION).ceil() as i64;

        // Collect keys and sizes of entries to evict.
        let mut stmt = db
            .prepare(
                "SELECT cache_key, size_bytes FROM media
                 ORDER BY hit_count ASC, last_hit_at ASC
                 LIMIT ?1",
            )
            .unwrap();

        let entries: Vec<(String, i64)> = stmt
            .query_map(params![evict_count], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();

        let mut freed: u64 = 0;
        for (key, size) in &entries {
            let _ = db.execute("DELETE FROM media WHERE cache_key = ?1", params![key]);
            freed += *size as u64;
        }

        self.inner.total_bytes.fetch_sub(freed, Ordering::Relaxed);

        let freed_mb = freed as f64 / (1024.0 * 1024.0);
        info!(
            evicted_count = entries.len(),
            freed_mb = format!("{:.1}", freed_mb),
            "Media cache eviction complete"
        );
    }
}

/// Current time as Unix epoch seconds.
fn epoch_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Character, CharacterData, CharacterTrait};

    fn make_test_char_data() -> CharacterData {
        let mut data = CharacterData::new();
        data.main.push(Character {
            id: "c1".to_string(),
            name: "Test Character".to_string(),
            name_original: "テスト".to_string(),
            role: "main".to_string(),
            sex: Some("f".to_string()),
            age: Some("17".to_string()),
            height: Some(160),
            weight: Some(50),
            blood_type: Some("A".to_string()),
            birthday: Some(vec![3, 14]),
            description: Some("A test character".to_string()),
            aliases: vec!["Testy".to_string()],
            personality: vec![CharacterTrait {
                name: "Kind".to_string(),
                spoiler: 0,
            }],
            roles: vec![],
            engages_in: vec![],
            subject_of: vec![],
            image_url: Some("https://example.com/img.jpg".to_string()),
            image_bytes: None,
            image_ext: None,
            image_width: None,
            image_height: None,
            first_name_hint: None,
            last_name_hint: None,
            seiyuu: None,
            seiyuu_image_url: None,
            seiyuu_image_bytes: None,
            seiyuu_image_ext: None,
            seiyuu_image_width: None,
            seiyuu_image_height: None,
        });
        data.side.push(Character {
            id: "c2".to_string(),
            name: "Side Char".to_string(),
            name_original: "サイド".to_string(),
            role: "side".to_string(),
            sex: None,
            age: None,
            height: None,
            weight: None,
            blood_type: None,
            birthday: None,
            description: None,
            aliases: vec![],
            personality: vec![],
            roles: vec![],
            engages_in: vec![],
            subject_of: vec![],
            image_url: None,
            image_bytes: None,
            image_ext: None,
            image_width: None,
            image_height: None,
            first_name_hint: None,
            last_name_hint: None,
            seiyuu: None,
            seiyuu_image_url: None,
            seiyuu_image_bytes: None,
            seiyuu_image_ext: None,
            seiyuu_image_width: None,
            seiyuu_image_height: None,
        });
        data
    }

    fn open_temp_cache() -> (MediaCache, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let cache = MediaCache::open(dir.path()).unwrap();
        (cache, dir)
    }

    #[test]
    fn test_put_and_get() {
        let (cache, _dir) = open_temp_cache();
        let data = make_test_char_data();

        cache.put("vndb:v17", "テストゲーム", &data);

        let entry = cache.get("vndb:v17").expect("Expected cache hit");
        assert_eq!(entry.title, "テストゲーム");
        assert_eq!(entry.char_data.all_characters().count(), 2);

        let main_char = &entry.char_data.main[0];
        assert_eq!(main_char.id, "c1");
        assert_eq!(main_char.name, "Test Character");
        assert_eq!(main_char.name_original, "テスト");
        assert!(main_char.image_bytes.is_none());
    }

    #[test]
    fn test_get_miss() {
        let (cache, _dir) = open_temp_cache();
        assert!(cache.get("vndb:v9999").is_none());
    }

    #[test]
    fn test_total_bytes_tracking() {
        let (cache, _dir) = open_temp_cache();
        let data1 = make_test_char_data();

        let mut data2 = CharacterData::new();
        data2.main.push(Character {
            id: "c10".to_string(),
            name: "Another".to_string(),
            name_original: "アナザー".to_string(),
            role: "main".to_string(),
            sex: None,
            age: None,
            height: None,
            weight: None,
            blood_type: None,
            birthday: None,
            description: Some("A longer description for size testing".to_string()),
            aliases: vec![],
            personality: vec![],
            roles: vec![],
            engages_in: vec![],
            subject_of: vec![],
            image_url: None,
            image_bytes: None,
            image_ext: None,
            image_width: None,
            image_height: None,
            first_name_hint: None,
            last_name_hint: None,
            seiyuu: None,
            seiyuu_image_url: None,
            seiyuu_image_bytes: None,
            seiyuu_image_ext: None,
            seiyuu_image_width: None,
            seiyuu_image_height: None,
        });

        cache.put("vndb:v1", "Title 1", &data1);
        let s1 = serde_json::to_vec(&data1).unwrap().len() as u64;

        cache.put("anilist:9253:ANIME", "Title 2", &data2);
        let s2 = serde_json::to_vec(&data2).unwrap().len() as u64;

        assert_eq!(cache.total_bytes(), s1 + s2);
    }

    #[test]
    fn test_hit_count_increments() {
        let (cache, _dir) = open_temp_cache();
        let data = make_test_char_data();

        cache.put("vndb:v17", "Title", &data);

        // 3 reads
        cache.get("vndb:v17");
        cache.get("vndb:v17");
        cache.get("vndb:v17");

        // Verify hit_count in DB
        let db = cache.inner.db.lock().unwrap();
        let hit_count: i64 = db
            .query_row(
                "SELECT hit_count FROM media WHERE cache_key = 'vndb:v17'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(hit_count, 3);
    }

    #[test]
    fn test_replace_preserves_hit_count() {
        let (cache, _dir) = open_temp_cache();
        let data = make_test_char_data();

        cache.put("vndb:v17", "Title", &data);
        cache.get("vndb:v17");
        cache.get("vndb:v17");

        // Re-put with updated data — hit_count should reset to 0 per UPSERT
        // (plan says "preserves" but the UPSERT doesn't carry forward hit_count
        //  because INSERT sets hit_count=0 and ON CONFLICT doesn't update it;
        //  actually the ON CONFLICT DO UPDATE doesn't touch hit_count, so the
        //  existing hit_count is preserved because it's not in the SET clause)
        let mut updated = make_test_char_data();
        updated.main[0].name = "Updated Name".to_string();
        cache.put("vndb:v17", "New Title", &updated);

        let db = cache.inner.db.lock().unwrap();
        let hit_count: i64 = db
            .query_row(
                "SELECT hit_count FROM media WHERE cache_key = 'vndb:v17'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(hit_count, 2);

        // Verify data was actually updated
        drop(db);
        let entry = cache.get("vndb:v17").unwrap();
        assert_eq!(entry.title, "New Title");
        assert_eq!(entry.char_data.main[0].name, "Updated Name");
    }

    #[test]
    fn test_replace_adjusts_total_bytes() {
        let (cache, _dir) = open_temp_cache();

        // Small entry
        let mut small = CharacterData::new();
        small.main.push(Character {
            id: "c1".to_string(),
            name: "A".to_string(),
            name_original: "A".to_string(),
            role: "main".to_string(),
            sex: None,
            age: None,
            height: None,
            weight: None,
            blood_type: None,
            birthday: None,
            description: None,
            aliases: vec![],
            personality: vec![],
            roles: vec![],
            engages_in: vec![],
            subject_of: vec![],
            image_url: None,
            image_bytes: None,
            image_ext: None,
            image_width: None,
            image_height: None,
            first_name_hint: None,
            last_name_hint: None,
            seiyuu: None,
            seiyuu_image_url: None,
            seiyuu_image_bytes: None,
            seiyuu_image_ext: None,
            seiyuu_image_width: None,
            seiyuu_image_height: None,
        });

        cache.put("vndb:v1", "T", &small);
        let small_size = serde_json::to_vec(&small).unwrap().len() as u64;
        assert_eq!(cache.total_bytes(), small_size);

        // Replace with larger entry
        let large = make_test_char_data();
        cache.put("vndb:v1", "T", &large);
        let large_size = serde_json::to_vec(&large).unwrap().len() as u64;
        assert_eq!(cache.total_bytes(), large_size);

        // Replace back with small
        cache.put("vndb:v1", "T", &small);
        assert_eq!(cache.total_bytes(), small_size);
    }

    #[test]
    fn test_ttl_expiry() {
        let (cache, _dir) = open_temp_cache();
        let data = make_test_char_data();
        let data_bytes = serde_json::to_vec(&data).unwrap();
        let size_bytes = data_bytes.len() as i64;

        // Insert directly with created_at 31 days ago.
        let old_time = epoch_secs() - (31 * 24 * 60 * 60);
        {
            let db = cache.inner.db.lock().unwrap();
            db.execute(
                "INSERT INTO media (cache_key, title, data, size_bytes, created_at, hit_count, last_hit_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, 0, ?5)",
                params!["vndb:v99", "Old", data_bytes, size_bytes, old_time],
            )
            .unwrap();
        }
        cache
            .inner
            .total_bytes
            .fetch_add(size_bytes as u64, Ordering::Relaxed);

        let before = cache.total_bytes();
        assert!(cache.get("vndb:v99").is_none());

        // Row should be deleted and total_bytes adjusted.
        assert_eq!(cache.total_bytes(), before - size_bytes as u64);

        let db = cache.inner.db.lock().unwrap();
        let count: i64 = db
            .query_row(
                "SELECT COUNT(*) FROM media WHERE cache_key = 'vndb:v99'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn test_corrupt_data_cleanup() {
        let (cache, _dir) = open_temp_cache();

        // Insert corrupt BLOB directly.
        let garbage = b"not valid json at all";
        let size = garbage.len() as i64;
        let now = epoch_secs();
        {
            let db = cache.inner.db.lock().unwrap();
            db.execute(
                "INSERT INTO media (cache_key, title, data, size_bytes, created_at, hit_count, last_hit_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, 0, ?5)",
                params!["vndb:v666", "Corrupt", garbage.as_slice(), size, now],
            )
            .unwrap();
        }
        cache
            .inner
            .total_bytes
            .fetch_add(size as u64, Ordering::Relaxed);

        let before = cache.total_bytes();
        assert!(cache.get("vndb:v666").is_none());

        // Should be cleaned up.
        assert_eq!(cache.total_bytes(), before - size as u64);

        let db = cache.inner.db.lock().unwrap();
        let count: i64 = db
            .query_row(
                "SELECT COUNT(*) FROM media WHERE cache_key = 'vndb:v666'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
    }

    // ===== Additional comprehensive tests =====

    #[test]
    fn test_multiple_entries_independent() {
        let (cache, _dir) = open_temp_cache();
        let data1 = make_test_char_data();
        let mut data2 = CharacterData::new();
        data2.main.push(Character {
            id: "c10".to_string(),
            name: "Other".to_string(),
            name_original: "その他".to_string(),
            role: "main".to_string(),
            sex: None,
            age: None,
            height: None,
            weight: None,
            blood_type: None,
            birthday: None,
            description: None,
            aliases: vec![],
            personality: vec![],
            roles: vec![],
            engages_in: vec![],
            subject_of: vec![],
            image_url: None,
            image_bytes: None,
            image_ext: None,
            image_width: None,
            image_height: None,
            first_name_hint: None,
            last_name_hint: None,
            seiyuu: None,
            seiyuu_image_url: None,
            seiyuu_image_bytes: None,
            seiyuu_image_ext: None,
            seiyuu_image_width: None,
            seiyuu_image_height: None,
        });

        cache.put("vndb:v17", "Title 1", &data1);
        cache.put("anilist:9253:ANIME", "Title 2", &data2);

        let e1 = cache.get("vndb:v17").unwrap();
        let e2 = cache.get("anilist:9253:ANIME").unwrap();

        assert_eq!(e1.title, "Title 1");
        assert_eq!(e1.char_data.all_characters().count(), 2);
        assert_eq!(e2.title, "Title 2");
        assert_eq!(e2.char_data.all_characters().count(), 1);
    }

    #[test]
    fn test_cache_reopen_preserves_data() {
        let dir = tempfile::tempdir().unwrap();
        let data = make_test_char_data();

        {
            let cache = MediaCache::open(dir.path()).unwrap();
            cache.put("vndb:v17", "Persistent", &data);
        }

        {
            let cache = MediaCache::open(dir.path()).unwrap();
            let entry = cache.get("vndb:v17").unwrap();
            assert_eq!(entry.title, "Persistent");
            assert_eq!(entry.char_data.main[0].id, "c1");
        }
    }

    #[test]
    fn test_total_bytes_after_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let data = make_test_char_data();
        let expected_size = serde_json::to_vec(&data).unwrap().len() as u64;

        {
            let cache = MediaCache::open(dir.path()).unwrap();
            cache.put("vndb:v17", "T", &data);
            assert_eq!(cache.total_bytes(), expected_size);
        }

        {
            let cache = MediaCache::open(dir.path()).unwrap();
            assert_eq!(cache.total_bytes(), expected_size);
        }
    }

    #[test]
    fn test_empty_character_data_cached() {
        let (cache, _dir) = open_temp_cache();
        let empty = CharacterData::new();

        cache.put("vndb:v999", "Empty Game", &empty);
        let entry = cache.get("vndb:v999").unwrap();
        assert_eq!(entry.title, "Empty Game");
        assert_eq!(entry.char_data.all_characters().count(), 0);
    }

    #[test]
    fn test_unicode_title_preserved() {
        let (cache, _dir) = open_temp_cache();
        let data = make_test_char_data();

        cache.put("vndb:v17", "シュタインズ・ゲート", &data);
        let entry = cache.get("vndb:v17").unwrap();
        assert_eq!(entry.title, "シュタインズ・ゲート");
    }

    #[test]
    fn test_cache_key_with_special_chars() {
        let (cache, _dir) = open_temp_cache();
        let data = make_test_char_data();

        cache.put("anilist:9253:ANIME", "Test", &data);
        assert!(cache.get("anilist:9253:ANIME").is_some());
        assert!(cache.get("anilist:9253:MANGA").is_none());
    }

    #[test]
    fn test_fresh_entry_not_expired() {
        let (cache, _dir) = open_temp_cache();
        let data = make_test_char_data();

        cache.put("vndb:v17", "Fresh", &data);
        // Should be retrievable immediately (not expired)
        assert!(cache.get("vndb:v17").is_some());
        assert!(cache.get("vndb:v17").is_some());
        assert!(cache.get("vndb:v17").is_some());
    }
}
