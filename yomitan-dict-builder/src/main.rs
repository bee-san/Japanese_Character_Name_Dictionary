use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse,
    },
    routing::get,
    Router,
};
use futures::stream::{self, StreamExt};
use moka::future::Cache;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio_stream::wrappers::ReceiverStream;
use tower_http::services::ServeDir;
use tracing::{info, warn};

mod anilist_client;
mod content_builder;
mod dict_builder;
mod disk_cache;
mod image_handler;
mod models;
mod name_parser;
mod vndb_client;

use anilist_client::AnilistClient;
use dict_builder::DictBuilder;
use disk_cache::DiskImageCache;
use image_handler::ImageHandler;
use models::UserMediaEntry;
use vndb_client::VndbClient;

/// Returns the path to the `static` directory.
///
/// In debug builds (i.e. `cargo run`), uses the compile-time
/// `CARGO_MANIFEST_DIR` so the binary finds `static/` regardless of the
/// working directory.  In release builds (Docker / production) falls back
/// to a plain relative `"static"` path, which works because the Dockerfile
/// sets `WORKDIR /app` and copies `static/` there.
fn static_dir() -> std::path::PathBuf {
    if cfg!(debug_assertions) {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("static")
    } else {
        std::path::PathBuf::from("static")
    }
}

/// Shared application state for temporary ZIP storage.
/// Maps token to (zip_bytes, creation_time).
type DownloadStore = Arc<Mutex<HashMap<String, (Vec<u8>, std::time::Instant)>>>;

/// Disk-backed image cache entry: (resized_bytes, extension).
type ImageCacheEntry = (Vec<u8>, String);

/// In-memory ZIP cache entry: completed ZIP bytes.
type ZipCacheEntry = Vec<u8>;

#[derive(Clone)]
struct AppState {
    downloads: DownloadStore,
    /// Shared HTTP client for connection pooling across all API calls.
    http_client: reqwest::Client,
    /// In-memory image cache: URL → (resized_bytes, extension).
    /// Weighted by byte size with 500MB cap, 24h TTL.
    /// Backed by disk cache for persistence across restarts.
    image_cache: Cache<String, ImageCacheEntry>,
    /// Disk-backed image cache with 30-day TTL. Survives process restarts.
    disk_image_cache: DiskImageCache,
    /// ZIP cache: query_key → zip_bytes. Short TTL for username-based, longer for single-media.
    zip_cache: Cache<String, ZipCacheEntry>,
}

impl AppState {
    async fn new() -> Self {
        let cache_dir = std::env::var("CACHE_DIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| {
                // Default: ./cache in debug, /var/cache/yomitan in release
                if cfg!(debug_assertions) {
                    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("cache")
                } else {
                    std::path::PathBuf::from("/var/cache/yomitan")
                }
            });

        let disk_image_cache = DiskImageCache::new(cache_dir.join("images")).await;
        disk_image_cache.spawn_cleanup_task();

        Self {
            downloads: Arc::new(Mutex::new(HashMap::new())),
            http_client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .expect("Failed to build HTTP client"),
            // Image cache: weighted by byte size, 500MB cap, 24h TTL
            image_cache: Cache::builder()
                .weigher(|_key: &String, value: &ImageCacheEntry| -> u32 {
                    // Weight = image bytes + extension string + overhead estimate
                    (value.0.len() + value.1.len() + 64).min(u32::MAX as usize) as u32
                })
                .max_capacity(500 * 1024 * 1024) // 500MB actual byte budget
                .time_to_live(std::time::Duration::from_secs(86400))
                .build(),
            disk_image_cache,
            // ZIP cache: max 200 entries, 15min TTL
            zip_cache: Cache::builder()
                .max_capacity(200)
                .time_to_live(std::time::Duration::from_secs(900))
                .build(),
        }
    }
}

// === Query parameter structs ===

#[derive(Deserialize)]
struct DictQuery {
    source: Option<String>,    // "vndb" or "anilist" (for single-media mode)
    id: Option<String>,        // VN ID like "v17" or AniList media ID (for single-media mode)
    #[serde(default)]
    spoiler_level: u8,
    #[serde(default = "default_media_type")]
    media_type: String,        // "ANIME" or "MANGA" (for AniList single-media)
    vndb_user: Option<String>,    // VNDB username (for username mode)
    anilist_user: Option<String>, // AniList username (for username mode)
    #[serde(default = "default_honorifics")]
    honorifics: bool,             // Generate honorific suffix entries (default true)
}

#[derive(Deserialize)]
struct UserListQuery {
    vndb_user: Option<String>,
    anilist_user: Option<String>,
}

#[derive(Deserialize)]
struct GenerateStreamQuery {
    vndb_user: Option<String>,
    anilist_user: Option<String>,
    #[serde(default)]
    spoiler_level: u8,
    #[serde(default = "default_honorifics")]
    honorifics: bool,
}

#[derive(Deserialize)]
struct DownloadQuery {
    token: String,
}

fn default_media_type() -> String {
    "ANIME".to_string()
}

fn default_honorifics() -> bool {
    true
}

/// Build a cache key for ZIP caching from query parameters.
fn zip_cache_key(
    source: Option<&str>,
    id: Option<&str>,
    vndb_user: &str,
    anilist_user: &str,
    spoiler_level: u8,
    honorifics: bool,
    media_type: &str,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(source.unwrap_or(""));
    hasher.update(id.unwrap_or(""));
    hasher.update(vndb_user);
    hasher.update(anilist_user);
    hasher.update(spoiler_level.to_string());
    hasher.update(honorifics.to_string());
    hasher.update(media_type);
    format!("{:x}", hasher.finalize())
}

/// Get the base URL for auto-update URLs.
/// Reads from BASE_URL env var, defaults to http://127.0.0.1:3000.
fn base_url() -> String {
    std::env::var("BASE_URL").unwrap_or_else(|_| {
        let port = std::env::var("PORT")
            .ok()
            .and_then(|p| p.parse::<u16>().ok())
            .unwrap_or(3000);
        format!("http://127.0.0.1:{}", port)
    })
}

#[tokio::main]
async fn main() {
    // Initialize structured logging
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let state = AppState::new().await;

    let app = Router::new()
        .route("/", get(serve_index))
        .route("/api/user-lists", get(fetch_user_lists))
        .route("/api/generate-stream", get(generate_stream))
        .route("/api/download", get(download_zip))
        .route("/api/yomitan-dict", get(generate_dict))
        .route("/api/yomitan-index", get(generate_index))
        .nest_service("/static", ServeDir::new(static_dir()))
        .with_state(state);

    let port = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse::<u16>().ok())
        .unwrap_or(3000);
    let host = std::env::var("HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
    let addr = format!("{}:{}", host, port);
    info!("Server running on http://{}", addr);

    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

async fn serve_index() -> impl IntoResponse {
    let path = static_dir().join("index.html");
    match tokio::fs::read(&path).await {
        Ok(bytes) => (
            StatusCode::OK,
            [("content-type", "text/html; charset=utf-8")],
            bytes,
        )
            .into_response(),
        Err(_) => (StatusCode::NOT_FOUND, "index.html not found").into_response(),
    }
}

// === Fetch user lists endpoint ===

async fn fetch_user_lists(
    Query(params): Query<UserListQuery>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let vndb_user = params.vndb_user.as_deref().unwrap_or("").trim().to_string();
    let anilist_user = params.anilist_user.as_deref().unwrap_or("").trim().to_string();

    if vndb_user.is_empty() && anilist_user.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            [("content-type", "application/json"), ("access-control-allow-origin", "*")],
            r#"{"error":"At least one username (vndb_user or anilist_user) is required"}"#.to_string(),
        )
            .into_response();
    }

    let mut all_entries: Vec<UserMediaEntry> = Vec::new();
    let mut errors: Vec<String> = Vec::new();

    if !vndb_user.is_empty() {
        let client = VndbClient::with_client(state.http_client.clone());
        match client.fetch_user_playing_list(&vndb_user).await {
            Ok(entries) => all_entries.extend(entries),
            Err(e) => errors.push(format!("VNDB: {}", e)),
        }
    }

    if !anilist_user.is_empty() {
        let client = AnilistClient::with_client(state.http_client.clone());
        match client.fetch_user_current_list(&anilist_user).await {
            Ok(entries) => all_entries.extend(entries),
            Err(e) => errors.push(format!("AniList: {}", e)),
        }
    }

    if all_entries.is_empty() && !errors.is_empty() {
        let error_msg = errors.join("; ");
        return (
            StatusCode::BAD_REQUEST,
            [("content-type", "application/json"), ("access-control-allow-origin", "*")],
            serde_json::json!({"error": error_msg}).to_string(),
        )
            .into_response();
    }

    let response = serde_json::json!({
        "entries": all_entries,
        "errors": errors,
        "count": all_entries.len()
    });

    (
        StatusCode::OK,
        [("content-type", "application/json"), ("access-control-allow-origin", "*")],
        response.to_string(),
    )
        .into_response()
}

// === SSE progress stream endpoint ===

async fn generate_stream(
    Query(params): Query<GenerateStreamQuery>,
    State(state): State<AppState>,
) -> Sse<ReceiverStream<Result<Event, std::convert::Infallible>>> {
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event, std::convert::Infallible>>(100);
    let spoiler_level = params.spoiler_level.min(2);
    let vndb_user = params.vndb_user.unwrap_or_default().trim().to_string();
    let anilist_user = params.anilist_user.unwrap_or_default().trim().to_string();
    let honorifics = params.honorifics;

    tokio::spawn(async move {
        let result = generate_dict_from_usernames(
            &vndb_user,
            &anilist_user,
            spoiler_level,
            honorifics,
            Some(&tx),
            &state,
        )
        .await;

        match result {
            Ok(zip_bytes) => {
                let token = uuid::Uuid::new_v4().to_string();
                {
                    let mut store = state.downloads.lock().await;
                    let now = std::time::Instant::now();
                    store.retain(|_, (_, created)| now.duration_since(*created).as_secs() < 300);
                    store.insert(token.clone(), (zip_bytes, now));
                }
                let _ = tx
                    .send(Ok(Event::default()
                        .event("complete")
                        .data(serde_json::json!({"token": token}).to_string())))
                    .await;
            }
            Err(e) => {
                let _ = tx
                    .send(Ok(Event::default()
                        .event("error")
                        .data(serde_json::json!({"error": e}).to_string())))
                    .await;
            }
        }
    });

    Sse::new(ReceiverStream::new(rx)).keep_alive(KeepAlive::default())
}

// === Download completed ZIP by token ===

async fn download_zip(
    Query(params): Query<DownloadQuery>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let mut store = state.downloads.lock().await;

    if let Some((zip_bytes, _)) = store.remove(&params.token) {
        (
            StatusCode::OK,
            [
                ("content-type", "application/zip"),
                ("content-disposition", "attachment; filename=bee_characters.zip"),
                ("access-control-allow-origin", "*"),
            ],
            zip_bytes,
        )
            .into_response()
    } else {
        (StatusCode::NOT_FOUND, "Download token not found or expired").into_response()
    }
}

/// Download, resize, and cache a single character image.
/// Returns (resized_bytes, extension) or None on failure.
///
/// Lookup order: moka (memory) → disk → HTTP.
/// On HTTP fetch, writes to both moka and disk.
/// On disk hit, promotes to moka for fast subsequent access.
async fn fetch_and_cache_image(
    url: &str,
    http_client: &reqwest::Client,
    image_cache: &Cache<String, ImageCacheEntry>,
    disk_cache: &DiskImageCache,
) -> Option<ImageCacheEntry> {
    // Tier 1: in-memory cache
    if let Some(cached) = image_cache.get(url).await {
        return Some(cached);
    }

    // Tier 2: disk cache (promotes to memory on hit)
    if let Some((bytes, ext)) = disk_cache.get(url).await {
        let entry: ImageCacheEntry = (bytes, ext);
        image_cache.insert(url.to_string(), entry.clone()).await;
        return Some(entry);
    }

    // Tier 3: HTTP download with per-image timeout
    let download_future = async {
        let response = http_client.get(url).send().await.ok()?;
        if response.status() != 200 {
            warn!(url = url, status = %response.status(), "Image download returned non-200");
            return None;
        }
        response.bytes().await.ok()
    };

    let raw_bytes = match tokio::time::timeout(
        std::time::Duration::from_secs(10),
        download_future,
    ).await {
        Ok(Some(bytes)) => bytes,
        Ok(None) => return None,
        Err(_) => {
            warn!(url = url, "Image download timed out after 10s");
            return None;
        }
    };

    // Resize to thumbnail + convert to WebP
    let (resized, ext) = ImageHandler::resize_image(&raw_bytes);

    let entry: ImageCacheEntry = (resized, ext.to_string());

    // Write to both cache tiers
    image_cache.insert(url.to_string(), entry.clone()).await;
    disk_cache.put(url, &entry.0, &entry.1).await;

    Some(entry)
}

/// Download images for all characters concurrently, with resize + caching.
/// Concurrency is capped to respect API rate limits.
async fn download_images_concurrent(
    char_data: &mut models::CharacterData,
    http_client: &reqwest::Client,
    image_cache: &Cache<String, ImageCacheEntry>,
    disk_cache: &DiskImageCache,
    concurrency: usize,
) {
    // Collect (index_in_flat_list, url) pairs
    let all_chars: Vec<_> = char_data.all_characters().enumerate().collect();
    let urls: Vec<(usize, String)> = all_chars
        .iter()
        .filter_map(|(i, c)| c.image_url.as_ref().map(|url| (*i, url.clone())))
        .collect();

    // Download concurrently
    let results: Vec<(usize, Option<ImageCacheEntry>)> = stream::iter(urls)
        .map(|(idx, url)| {
            let client = http_client.clone();
            let cache = image_cache.clone();
            let disk = disk_cache.clone();
            async move {
                let result = fetch_and_cache_image(&url, &client, &cache, &disk).await;
                (idx, result)
            }
        })
        .buffer_unordered(concurrency)
        .collect()
        .await;

    // Apply results back to characters
    let mut flat: Vec<&mut models::Character> = char_data.all_characters_mut().collect();
    for (idx, result) in results {
        if let Some((bytes, ext)) = result {
            if let Some(ch) = flat.get_mut(idx) {
                ch.image_bytes = Some(bytes);
                ch.image_ext = Some(ext);
            }
        }
    }
}

// === Core function: Generate dictionary from usernames ===

async fn generate_dict_from_usernames(
    vndb_user: &str,
    anilist_user: &str,
    spoiler_level: u8,
    honorifics: bool,
    progress_tx: Option<&tokio::sync::mpsc::Sender<Result<Event, std::convert::Infallible>>>,
    state: &AppState,
) -> Result<Vec<u8>, String> {
    let spoiler_level = spoiler_level.min(2);

    // Check ZIP cache (skip for SSE streaming — user expects fresh progress)
    if progress_tx.is_none() {
        let cache_key = zip_cache_key(None, None, vndb_user, anilist_user, spoiler_level, honorifics, "");
        if let Some(cached) = state.zip_cache.get(&cache_key).await {
            return Ok(cached);
        }
    }

    // Step 1: Collect all media entries from user lists
    let mut media_entries: Vec<UserMediaEntry> = Vec::new();

    if !vndb_user.is_empty() {
        let client = VndbClient::with_client(state.http_client.clone());
        match client.fetch_user_playing_list(vndb_user).await {
            Ok(entries) => media_entries.extend(entries),
            Err(e) => {
                if anilist_user.is_empty() {
                    return Err(format!("VNDB error: {}", e));
                }
                warn!(user = vndb_user, error = %e, "VNDB list fetch error (continuing)");
            }
        }
    }

    if !anilist_user.is_empty() {
        let client = AnilistClient::with_client(state.http_client.clone());
        match client.fetch_user_current_list(anilist_user).await {
            Ok(entries) => media_entries.extend(entries),
            Err(e) => {
                if vndb_user.is_empty() || media_entries.is_empty() {
                    return Err(format!("AniList error: {}", e));
                }
                warn!(user = anilist_user, error = %e, "AniList list fetch error (continuing)");
            }
        }
    }

    if media_entries.is_empty() {
        return Err("No in-progress media found in user lists".to_string());
    }

    let total = media_entries.len();

    // Build download URL with usernames for auto-update (percent-encoded)
    let base = base_url();
    let mut url_parts = Vec::new();
    if !vndb_user.is_empty() {
        url_parts.push(format!("vndb_user={}", urlencoding::encode(vndb_user)));
    }
    if !anilist_user.is_empty() {
        url_parts.push(format!("anilist_user={}", urlencoding::encode(anilist_user)));
    }
    url_parts.push(format!("spoiler_level={}", spoiler_level));
    if !honorifics {
        url_parts.push("honorifics=false".to_string());
    }
    let download_url = format!(
        "{}/api/yomitan-dict?{}",
        base,
        url_parts.join("&")
    );

    let description = format!("Character Dictionary ({} titles)", total);

    let mut builder = DictBuilder::new(
        spoiler_level,
        Some(download_url),
        description,
        honorifics,
    );

    // Step 2: For each media, fetch characters and add to dictionary
    for (i, entry) in media_entries.iter().enumerate() {
        let display_title = if !entry.title_romaji.is_empty() {
            &entry.title_romaji
        } else {
            &entry.title
        };

        if let Some(tx) = progress_tx {
            let _ = tx
                .send(Ok(Event::default().event("progress").data(
                    serde_json::json!({
                        "current": i + 1,
                        "total": total,
                        "title": display_title
                    })
                    .to_string(),
                )))
                .await;
        }

        let game_title = &entry.title;

        match entry.source.as_str() {
            "vndb" => {
                let client = VndbClient::with_client(state.http_client.clone());

                let title = match client.fetch_vn_title(&entry.id).await {
                    Ok((romaji, original)) => {
                        if !original.is_empty() { original } else { romaji }
                    }
                    Err(_) => game_title.clone(),
                };

                match client.fetch_characters(&entry.id).await {
                    Ok(mut char_data) => {
                        // Concurrent image downloads with caching + resize
                        download_images_concurrent(
                            &mut char_data,
                            &state.http_client,
                            &state.image_cache,
                            &state.disk_image_cache,
                            8,
                        ).await;

                        for character in char_data.all_characters() {
                            builder.add_character(character, &title);
                        }
                    }
                    Err(e) => {
                        warn!(vn_id = %entry.id, error = %e, "Failed to fetch VNDB characters");
                    }
                }

                tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
            }
            "anilist" => {
                let client = AnilistClient::with_client(state.http_client.clone());
                let media_id: i32 = match entry.id.parse() {
                    Ok(id) => id,
                    Err(_) => {
                        warn!(id = %entry.id, "Invalid AniList media ID");
                        continue;
                    }
                };

                let media_type = match entry.media_type.as_str() {
                    "anime" => "ANIME",
                    "manga" => "MANGA",
                    _ => "ANIME",
                };

                match client.fetch_characters(media_id, media_type).await {
                    Ok((mut char_data, media_title)) => {
                        let title = if !media_title.is_empty() {
                            media_title
                        } else {
                            game_title.clone()
                        };

                        download_images_concurrent(
                            &mut char_data,
                            &state.http_client,
                            &state.image_cache,
                            &state.disk_image_cache,
                            6,
                        ).await;

                        for character in char_data.all_characters() {
                            builder.add_character(character, &title);
                        }
                    }
                    Err(e) => {
                        warn!(media_id = %entry.id, error = %e, "Failed to fetch AniList characters");
                    }
                }

                tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;
            }
            _ => {
                warn!(source = %entry.source, "Unknown source");
            }
        }
    }

    if builder.entries.is_empty() {
        return Err("No character entries generated from any media".to_string());
    }

    let zip_bytes = builder.export_bytes()?;

    // Cache the result
    let cache_key = zip_cache_key(None, None, vndb_user, anilist_user, spoiler_level, honorifics, "");
    state.zip_cache.insert(cache_key, zip_bytes.clone()).await;

    Ok(zip_bytes)
}

// === Generate dictionary (single media OR username-based) ===

async fn generate_dict(
    Query(params): Query<DictQuery>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let spoiler_level = params.spoiler_level.min(2);

    let vndb_user = params.vndb_user.as_deref().unwrap_or("").trim().to_string();
    let anilist_user = params.anilist_user.as_deref().unwrap_or("").trim().to_string();

    if !vndb_user.is_empty() || !anilist_user.is_empty() {
        match generate_dict_from_usernames(&vndb_user, &anilist_user, spoiler_level, params.honorifics, None, &state).await {
            Ok(bytes) => {
                return (
                    StatusCode::OK,
                    [
                        ("content-type", "application/zip"),
                        ("content-disposition", "attachment; filename=bee_characters.zip"),
                        ("access-control-allow-origin", "*"),
                    ],
                    bytes,
                )
                    .into_response();
            }
            Err(e) => {
                return (StatusCode::INTERNAL_SERVER_ERROR, e).into_response();
            }
        }
    }

    // Single-media mode
    let source = params.source.as_deref().unwrap_or("");
    let id = params.id.as_deref().unwrap_or("");

    if source.is_empty() || id.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            "Either provide source+id or vndb_user/anilist_user",
        )
            .into_response();
    }

    // Check ZIP cache
    let cache_key = zip_cache_key(Some(source), Some(id), "", "", spoiler_level, params.honorifics, &params.media_type);
    if let Some(cached) = state.zip_cache.get(&cache_key).await {
        return (
            StatusCode::OK,
            [
                ("content-type", "application/zip"),
                ("content-disposition", "attachment; filename=bee_characters.zip"),
                ("access-control-allow-origin", "*"),
            ],
            cached,
        )
            .into_response();
    }

    let download_url = {
        let base = base_url();
        format!(
            "{}/api/yomitan-dict?source={}&id={}&spoiler_level={}&media_type={}{}",
            base,
            urlencoding::encode(source),
            urlencoding::encode(id),
            spoiler_level,
            urlencoding::encode(&params.media_type),
            if !params.honorifics { "&honorifics=false" } else { "" }
        )
    };

    let result = match source.to_lowercase().as_str() {
        "vndb" => generate_vndb_dict(id, spoiler_level, params.honorifics, &download_url, &state).await,
        "anilist" => {
            let media_id: i32 = match id.parse() {
                Ok(id) => id,
                Err(_) => {
                    return (StatusCode::BAD_REQUEST, "Invalid AniList ID: must be a number").into_response()
                }
            };
            let media_type = params.media_type.to_uppercase();
            if media_type != "ANIME" && media_type != "MANGA" {
                return (StatusCode::BAD_REQUEST, "media_type must be ANIME or MANGA").into_response();
            }
            generate_anilist_dict(media_id, &media_type, spoiler_level, params.honorifics, &download_url, &state).await
        }
        _ => return (StatusCode::BAD_REQUEST, "source must be 'vndb' or 'anilist'").into_response(),
    };

    match result {
        Ok(bytes) => {
            // Cache the result
            state.zip_cache.insert(cache_key, bytes.clone()).await;
            (
                StatusCode::OK,
                [
                    ("content-type", "application/zip"),
                    ("content-disposition", "attachment; filename=bee_characters.zip"),
                    ("access-control-allow-origin", "*"),
                ],
                bytes,
            )
                .into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
    }
}

/// Lightweight endpoint: returns just the index.json metadata as JSON.
async fn generate_index(Query(params): Query<DictQuery>) -> impl IntoResponse {
    let spoiler_level = params.spoiler_level.min(2);

    let vndb_user = params.vndb_user.as_deref().unwrap_or("").trim().to_string();
    let anilist_user = params.anilist_user.as_deref().unwrap_or("").trim().to_string();

    let download_url = if !vndb_user.is_empty() || !anilist_user.is_empty() {
        let base = base_url();
        let mut url_parts = Vec::new();
        if !vndb_user.is_empty() {
            url_parts.push(format!("vndb_user={}", urlencoding::encode(&vndb_user)));
        }
        if !anilist_user.is_empty() {
            url_parts.push(format!("anilist_user={}", urlencoding::encode(&anilist_user)));
        }
        url_parts.push(format!("spoiler_level={}", spoiler_level));
        if !params.honorifics {
            url_parts.push("honorifics=false".to_string());
        }
        format!(
            "{}/api/yomitan-dict?{}",
            base,
            url_parts.join("&")
        )
    } else {
        let base = base_url();
        let source = params.source.as_deref().unwrap_or("");
        let id = params.id.as_deref().unwrap_or("");
        format!(
            "{}/api/yomitan-dict?source={}&id={}&spoiler_level={}&media_type={}{}",
            base,
            urlencoding::encode(source),
            urlencoding::encode(id),
            spoiler_level,
            urlencoding::encode(&params.media_type),
            if !params.honorifics { "&honorifics=false" } else { "" }
        )
    };

    let builder = DictBuilder::new(spoiler_level, Some(download_url), String::new(), params.honorifics);
    let index = builder.create_index_public();

    (
        StatusCode::OK,
        [
            ("content-type", "application/json"),
            ("access-control-allow-origin", "*"),
        ],
        serde_json::to_string(&index).unwrap(),
    )
        .into_response()
}

// === Single-media helpers ===

async fn generate_vndb_dict(
    vn_id: &str,
    spoiler_level: u8,
    honorifics: bool,
    download_url: &str,
    state: &AppState,
) -> Result<Vec<u8>, String> {
    let client = VndbClient::with_client(state.http_client.clone());

    let (romaji_title, original_title) = client
        .fetch_vn_title(vn_id)
        .await
        .unwrap_or_else(|_| ("Unknown VN".to_string(), String::new()));
    let game_title = if !original_title.is_empty() {
        original_title
    } else {
        romaji_title
    };

    let mut char_data = client.fetch_characters(vn_id).await?;

    // Concurrent image downloads with caching + resize
    download_images_concurrent(
        &mut char_data,
        &state.http_client,
        &state.image_cache,
        &state.disk_image_cache,
        8,
    ).await;

    let mut builder = DictBuilder::new(
        spoiler_level,
        Some(download_url.to_string()),
        game_title.clone(),
        honorifics,
    );

    for character in char_data.all_characters() {
        builder.add_character(character, &game_title);
    }

    if builder.entries.is_empty() {
        return Err("No character entries generated".to_string());
    }

    builder.export_bytes()
}

async fn generate_anilist_dict(
    media_id: i32,
    media_type: &str,
    spoiler_level: u8,
    honorifics: bool,
    download_url: &str,
    state: &AppState,
) -> Result<Vec<u8>, String> {
    let client = AnilistClient::with_client(state.http_client.clone());

    let (mut char_data, media_title) = client.fetch_characters(media_id, media_type).await?;

    let game_title = if !media_title.is_empty() {
        media_title
    } else {
        format!("AniList {}", media_id)
    };

    // Concurrent image downloads with caching + resize
    download_images_concurrent(
        &mut char_data,
        &state.http_client,
        &state.image_cache,
        &state.disk_image_cache,
        6,
    ).await;

    let mut builder = DictBuilder::new(
        spoiler_level,
        Some(download_url.to_string()),
        game_title.clone(),
        honorifics,
    );

    for character in char_data.all_characters() {
        builder.add_character(character, &game_title);
    }

    if builder.entries.is_empty() {
        return Err("No character entries generated".to_string());
    }

    builder.export_bytes()
}
