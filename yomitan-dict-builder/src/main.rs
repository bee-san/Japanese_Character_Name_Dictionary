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
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio_stream::wrappers::ReceiverStream;
use tower_governor::{
    governor::GovernorConfigBuilder, key_extractor::SmartIpKeyExtractor, GovernorLayer,
};
use tower_http::services::ServeDir;
use tracing::{error, info, warn};

mod anilist_client;
mod content_builder;
mod dict_builder;
mod image_cache;
mod image_handler;
mod kana;
mod media_cache;
mod models;
mod name_parser;
mod vndb_client;

#[cfg(test)]
mod anilist_name_test_data;

use anilist_client::AnilistClient;
use content_builder::DictSettings;
use dict_builder::DictBuilder;
use image_cache::ImageCache;
use image_handler::ImageHandler;
use media_cache::MediaCache;
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

/// Result of fetching an image: the index into the character list, and optionally the image bytes + extension.
type IndexedImageResult = (usize, Option<(Vec<u8>, String, u32, u32)>);

/// Interval for cleaning up expired download tokens.
const DOWNLOAD_CLEANUP_INTERVAL_SECS: u64 = 60;

/// Max age for download tokens (5 minutes).
const DOWNLOAD_TOKEN_MAX_AGE_SECS: u64 = 300;
const INVALID_INPUT_PREFIX: &str = "INVALID_INPUT:";
const RATE_LIMIT_PREFIX: &str = "RATE_LIMIT:";
const UPSTREAM_PREFIX: &str = "UPSTREAM:";

#[derive(Clone)]
struct AppState {
    downloads: DownloadStore,
    /// Shared HTTP client for connection pooling across all API calls.
    http_client: reqwest::Client,
    /// On-disk image cache with popularity-based eviction.
    image_cache: ImageCache,
    /// Per-media API response cache (character data + title).
    media_cache: MediaCache,
    /// Server start time for uptime reporting.
    started_at: std::time::Instant,
    /// Boot identifier for correlating lifecycle logs across restarts.
    boot_id: String,
}

impl AppState {
    fn new(boot_id: String) -> Self {
        let downloads: DownloadStore = Arc::new(Mutex::new(HashMap::new()));

        // Spawn periodic cleanup for download tokens
        {
            let dl = downloads.clone();
            tokio::spawn(async move {
                let interval = std::time::Duration::from_secs(DOWNLOAD_CLEANUP_INTERVAL_SECS);
                loop {
                    tokio::time::sleep(interval).await;
                    let mut store = dl.lock().await;
                    let now = std::time::Instant::now();
                    let before = store.len();
                    store.retain(|_, (_, created)| {
                        now.duration_since(*created).as_secs() < DOWNLOAD_TOKEN_MAX_AGE_SECS
                    });
                    let removed = before - store.len();
                    if removed > 0 {
                        info!(
                            removed = removed,
                            remaining = store.len(),
                            "Download token cleanup"
                        );
                    }
                }
            });
        }

        // Image cache directory: CACHE_DIR env or ./cache (debug) / /var/cache/yomitan (release)
        let cache_dir = std::env::var("CACHE_DIR").unwrap_or_else(|_| {
            if cfg!(debug_assertions) {
                "./cache".to_string()
            } else {
                "/var/cache/yomitan".to_string()
            }
        });
        let image_cache = ImageCache::open(std::path::Path::new(&cache_dir)).unwrap_or_else(|e| {
            error!(boot_id = %boot_id, error = %e, "Image cache initialization failed");
            std::process::exit(1)
        });
        let media_cache = MediaCache::open(std::path::Path::new(&cache_dir)).unwrap_or_else(|e| {
            error!(boot_id = %boot_id, error = %e, "Media cache initialization failed");
            std::process::exit(1)
        });

        let http_client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .unwrap_or_else(|e| {
                error!(boot_id = %boot_id, error = %e, "HTTP client initialization failed");
                std::process::exit(1)
            });

        Self {
            downloads,
            http_client,
            image_cache,
            media_cache,
            started_at: std::time::Instant::now(),
            boot_id,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ServiceErrorKind {
    InvalidInput,
    RateLimit,
    Upstream,
    Internal,
}

#[derive(Default)]
struct GenerationStats {
    media_total: usize,
    vndb_cache_hits: usize,
    vndb_cache_misses: usize,
    anilist_cache_hits: usize,
    anilist_cache_misses: usize,
    invalid_input_failures: usize,
    upstream_failures: usize,
    rate_limit_failures: usize,
}

impl GenerationStats {
    fn record_vndb_fetch(&mut self, cached: bool) {
        if cached {
            self.vndb_cache_hits += 1;
        } else {
            self.vndb_cache_misses += 1;
        }
    }

    fn record_anilist_fetch(&mut self, cached: bool) {
        if cached {
            self.anilist_cache_hits += 1;
        } else {
            self.anilist_cache_misses += 1;
        }
    }

    fn record_failure(&mut self, error: &str) {
        match classify_service_error_kind(error) {
            ServiceErrorKind::InvalidInput => self.invalid_input_failures += 1,
            ServiceErrorKind::RateLimit => self.rate_limit_failures += 1,
            ServiceErrorKind::Upstream => self.upstream_failures += 1,
            ServiceErrorKind::Internal => {}
        }
    }

    fn total_cache_hits(&self) -> usize {
        self.vndb_cache_hits + self.anilist_cache_hits
    }

    fn total_cache_misses(&self) -> usize {
        self.vndb_cache_misses + self.anilist_cache_misses
    }

    fn total_external_failures(&self) -> usize {
        self.invalid_input_failures + self.upstream_failures + self.rate_limit_failures
    }
}

#[derive(Clone)]
struct NormalizedManualEntry {
    source: String,
    id: String,
    media_type: String,
}

// === Query parameter structs ===

#[derive(Deserialize)]
struct DictQuery {
    source: Option<String>,  // "vndb" or "anilist" (for single-media mode)
    id: Option<String>,      // VN ID like "v17" or AniList media ID (for single-media mode)
    entries: Option<String>, // JSON array of {source, id, media_type?} for multi-entry mode
    #[serde(default = "default_media_type")]
    media_type: String, // "ANIME" or "MANGA" (for AniList single-media)
    vndb_user: Option<String>, // VNDB username (for username mode)
    anilist_user: Option<String>, // AniList username (for username mode)
    #[serde(default = "default_true")]
    honorifics: bool,
    #[serde(default = "default_true")]
    image: bool,
    #[serde(default = "default_true")]
    tag: bool,
    #[serde(default = "default_true")]
    description: bool,
    #[serde(default = "default_true")]
    traits: bool,
    #[serde(default = "default_true")]
    spoilers: bool,
    #[serde(default = "default_true")]
    seiyuu: bool,
}

impl DictQuery {
    fn to_settings(&self) -> DictSettings {
        DictSettings {
            show_image: self.image,
            show_tag: self.tag,
            show_description: self.description,
            show_traits: self.traits,
            show_spoilers: self.spoilers,
            honorifics: self.honorifics,
            show_seiyuu: self.seiyuu,
        }
    }

    /// Append non-default settings as query parameters to a URL parts list.
    fn append_settings_params(&self, parts: &mut Vec<String>) {
        if !self.honorifics {
            parts.push("honorifics=false".to_string());
        }
        if !self.image {
            parts.push("image=false".to_string());
        }
        if !self.tag {
            parts.push("tag=false".to_string());
        }
        if !self.description {
            parts.push("description=false".to_string());
        }
        if !self.traits {
            parts.push("traits=false".to_string());
        }
        if !self.spoilers {
            parts.push("spoilers=false".to_string());
        }
        if !self.seiyuu {
            parts.push("seiyuu=false".to_string());
        }
    }
}

/// A single entry in the `entries` JSON array for multi-entry manual mode.
#[derive(Deserialize)]
struct ManualEntry {
    source: String,
    id: String,
    #[serde(default = "default_media_type")]
    media_type: String,
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
    #[serde(default = "default_true")]
    honorifics: bool,
    #[serde(default = "default_true")]
    image: bool,
    #[serde(default = "default_true")]
    tag: bool,
    #[serde(default = "default_true")]
    description: bool,
    #[serde(default = "default_true")]
    traits: bool,
    #[serde(default = "default_true")]
    spoilers: bool,
    #[serde(default = "default_true")]
    seiyuu: bool,
}

impl GenerateStreamQuery {
    fn to_settings(&self) -> DictSettings {
        DictSettings {
            show_image: self.image,
            show_tag: self.tag,
            show_description: self.description,
            show_traits: self.traits,
            show_spoilers: self.spoilers,
            honorifics: self.honorifics,
            show_seiyuu: self.seiyuu,
        }
    }
}

#[derive(Deserialize)]
struct DownloadQuery {
    token: String,
}

fn default_media_type() -> String {
    "ANIME".to_string()
}

fn default_true() -> bool {
    true
}

/// Parse an AniList media ID from either a raw numeric string (e.g. "9253")
/// or an AniList URL (e.g. "https://anilist.co/anime/9253/..." or
/// "https://anilist.co/manga/30002").
/// Returns the numeric media ID on success.
fn parse_anilist_id(input: &str) -> Result<i32, String> {
    let input = input.trim();

    // Try to extract from AniList URL
    if input.contains("anilist.co/") {
        if let Some(pos) = input.rfind("anilist.co/") {
            let after = &input[pos + "anilist.co/".len()..];
            // Expected path: anime/9253 or manga/30002 (optionally followed by /slug, ?, #)
            let segments: Vec<&str> = after.split('/').collect();
            if segments.len() >= 2 {
                let id_segment = segments[1]
                    .split(&['?', '#'][..])
                    .next()
                    .unwrap_or("")
                    .trim();
                if let Ok(id) = id_segment.parse::<i32>() {
                    return Ok(id);
                }
            }
        }
        return Err(format!(
            "Could not extract a numeric media ID from AniList URL: {}",
            input
        ));
    }

    // Plain numeric ID
    input.parse::<i32>().map_err(|_| {
        format!(
            "Invalid AniList ID '{}': must be a number or AniList URL",
            input
        )
    })
}

fn classify_service_error_kind(error: &str) -> ServiceErrorKind {
    if error.starts_with(INVALID_INPUT_PREFIX) {
        ServiceErrorKind::InvalidInput
    } else if error.starts_with(RATE_LIMIT_PREFIX) {
        ServiceErrorKind::RateLimit
    } else if error.starts_with(UPSTREAM_PREFIX) {
        ServiceErrorKind::Upstream
    } else {
        ServiceErrorKind::Internal
    }
}

fn strip_service_error_prefix(error: &str) -> &str {
    error
        .strip_prefix(INVALID_INPUT_PREFIX)
        .or_else(|| error.strip_prefix(RATE_LIMIT_PREFIX))
        .or_else(|| error.strip_prefix(UPSTREAM_PREFIX))
        .map(str::trim)
        .unwrap_or(error)
}

fn status_code_for_error(error: &str) -> StatusCode {
    match classify_service_error_kind(error) {
        ServiceErrorKind::InvalidInput => StatusCode::BAD_REQUEST,
        ServiceErrorKind::RateLimit => StatusCode::SERVICE_UNAVAILABLE,
        ServiceErrorKind::Upstream => StatusCode::BAD_GATEWAY,
        ServiceErrorKind::Internal => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

fn public_error_message(error: &str) -> String {
    match classify_service_error_kind(error) {
        ServiceErrorKind::InvalidInput => strip_service_error_prefix(error).to_string(),
        ServiceErrorKind::RateLimit => {
            "Upstream service is temporarily rate limiting requests. Please try again shortly."
                .to_string()
        }
        ServiceErrorKind::Upstream => {
            "Failed to fetch data from an upstream service. Please try again shortly."
                .to_string()
        }
        ServiceErrorKind::Internal => "Internal server error".to_string(),
    }
}

fn aggregate_status_code(errors: &[String]) -> StatusCode {
    if errors
        .iter()
        .any(|error| classify_service_error_kind(error) == ServiceErrorKind::RateLimit)
    {
        StatusCode::SERVICE_UNAVAILABLE
    } else if errors
        .iter()
        .any(|error| classify_service_error_kind(error) == ServiceErrorKind::Upstream)
    {
        StatusCode::BAD_GATEWAY
    } else if !errors.is_empty()
        && errors
            .iter()
            .all(|error| classify_service_error_kind(error) == ServiceErrorKind::InvalidInput)
    {
        StatusCode::BAD_REQUEST
    } else {
        StatusCode::INTERNAL_SERVER_ERROR
    }
}

fn combine_service_errors(errors: &[String]) -> String {
    if errors.is_empty() {
        return "Internal server error".to_string();
    }

    let prefix = match aggregate_status_code(errors) {
        StatusCode::BAD_REQUEST => INVALID_INPUT_PREFIX,
        StatusCode::SERVICE_UNAVAILABLE => RATE_LIMIT_PREFIX,
        StatusCode::BAD_GATEWAY => UPSTREAM_PREFIX,
        _ => "",
    };

    let details = errors
        .iter()
        .map(|error| strip_service_error_prefix(error))
        .collect::<Vec<_>>()
        .join("; ");

    if prefix.is_empty() {
        details
    } else {
        format!("{} {}", prefix, details)
    }
}

fn normalize_vndb_user_input(input: &str) -> String {
    VndbClient::normalize_user_input(input)
}

fn normalize_anilist_user_input(input: &str) -> String {
    AnilistClient::normalize_user_input(input)
}

fn normalize_vndb_id_for_url(input: &str) -> String {
    VndbClient::parse_vn_id(input).unwrap_or_else(|_| input.trim().to_string())
}

fn normalize_anilist_id_for_url(input: &str) -> String {
    parse_anilist_id(input)
        .map(|id| id.to_string())
        .unwrap_or_else(|_| input.trim().to_string())
}

fn normalize_manual_entry(entry: &ManualEntry) -> Result<NormalizedManualEntry, String> {
    let source = entry.source.trim().to_lowercase();
    match source.as_str() {
        "vndb" => Ok(NormalizedManualEntry {
            source,
            id: VndbClient::parse_vn_id(&entry.id)
                .map_err(|e| format!("{} {}", INVALID_INPUT_PREFIX, e))?,
            media_type: "vn".to_string(),
        }),
        "anilist" => Ok(NormalizedManualEntry {
            source,
            id: parse_anilist_id(&entry.id)
                .map(|id| id.to_string())
                .map_err(|e| format!("{} {}", INVALID_INPUT_PREFIX, e))?,
            media_type: match entry.media_type.to_uppercase().as_str() {
                "MANGA" => "MANGA".to_string(),
                _ => "ANIME".to_string(),
            },
        }),
        _ => Err(format!(
            "{} source must be 'vndb' or 'anilist'",
            INVALID_INPUT_PREFIX
        )),
    }
}

fn normalize_entries_json_for_url(entries_json: &str) -> String {
    let entries = match serde_json::from_str::<Vec<ManualEntry>>(entries_json) {
        Ok(entries) => entries,
        Err(_) => return entries_json.to_string(),
    };

    let normalized_entries: Vec<serde_json::Value> = entries
        .iter()
        .filter_map(|entry| normalize_manual_entry(entry).ok())
        .map(|entry| {
            let mut value = serde_json::json!({
                "source": entry.source,
                "id": entry.id,
            });
            if entry.source == "anilist" {
                value["media_type"] = serde_json::json!(entry.media_type);
            }
            value
        })
        .collect();

    if normalized_entries.is_empty() {
        entries_json.to_string()
    } else {
        serde_json::to_string(&normalized_entries).unwrap_or_else(|_| entries_json.to_string())
    }
}

fn log_generation_summary(
    state: &AppState,
    mode: &str,
    started_at: std::time::Instant,
    stats: &GenerationStats,
    builder: Option<&DictBuilder>,
    zip_size_bytes: Option<usize>,
    error: Option<&str>,
) {
    let skipped_no_japanese_count = builder
        .map(DictBuilder::skipped_no_japanese_count)
        .unwrap_or(0);
    let duration_ms = started_at.elapsed().as_millis() as u64;
    let zip_size_bytes = zip_size_bytes.unwrap_or(0);

    match error {
        Some(error) => warn!(
            boot_id = %state.boot_id,
            mode = mode,
            media_total = stats.media_total,
            cache_hits = stats.total_cache_hits(),
            cache_misses = stats.total_cache_misses(),
            vndb_cache_hits = stats.vndb_cache_hits,
            vndb_cache_misses = stats.vndb_cache_misses,
            anilist_cache_hits = stats.anilist_cache_hits,
            anilist_cache_misses = stats.anilist_cache_misses,
            upstream_failures = stats.total_external_failures(),
            rate_limit_failures = stats.rate_limit_failures,
            invalid_input_failures = stats.invalid_input_failures,
            skipped_no_japanese_count = skipped_no_japanese_count,
            zip_size_bytes = zip_size_bytes,
            duration_ms = duration_ms,
            error = %error,
            "Dictionary generation failed"
        ),
        None => info!(
            boot_id = %state.boot_id,
            mode = mode,
            media_total = stats.media_total,
            cache_hits = stats.total_cache_hits(),
            cache_misses = stats.total_cache_misses(),
            vndb_cache_hits = stats.vndb_cache_hits,
            vndb_cache_misses = stats.vndb_cache_misses,
            anilist_cache_hits = stats.anilist_cache_hits,
            anilist_cache_misses = stats.anilist_cache_misses,
            upstream_failures = stats.total_external_failures(),
            rate_limit_failures = stats.rate_limit_failures,
            invalid_input_failures = stats.invalid_input_failures,
            skipped_no_japanese_count = skipped_no_japanese_count,
            zip_size_bytes = zip_size_bytes,
            duration_ms = duration_ms,
            "Dictionary generation completed"
        ),
    }
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

async fn shutdown_signal(boot_id: String) {
    let ctrl_c = async {
        if let Err(error) = tokio::signal::ctrl_c().await {
            error!(
                boot_id = %boot_id,
                error = %error,
                "Failed to install Ctrl+C signal handler"
            );
        }
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(error) => {
                error!(
                    boot_id = %boot_id,
                    error = %error,
                    "Failed to install SIGTERM signal handler"
                );
            }
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {
            info!(boot_id = %boot_id, "Received Ctrl+C shutdown signal");
        }
        _ = terminate => {
            info!(boot_id = %boot_id, "Received SIGTERM shutdown signal");
        }
    }
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

    let boot_id = uuid::Uuid::new_v4().to_string();

    {
        let panic_boot_id = boot_id.clone();
        std::panic::set_hook(Box::new(move |panic_info| {
            let location = panic_info
                .location()
                .map(|location| {
                    format!(
                        "{}:{}:{}",
                        location.file(),
                        location.line(),
                        location.column()
                    )
                })
                .unwrap_or_else(|| "unknown".to_string());
            let message = panic_info
                .payload()
                .downcast_ref::<&str>()
                .map(|message| (*message).to_string())
                .or_else(|| panic_info.payload().downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "non-string panic payload".to_string());

            error!(
                boot_id = %panic_boot_id,
                location = %location,
                message = %message,
                "Unhandled panic"
            );
        }));
    }

    let state = AppState::new(boot_id.clone());

    // Rate limiting: strict for expensive generation endpoints
    let generate_governor = GovernorConfigBuilder::default()
        .per_second(20) // replenish 1 token every 20 seconds
        .burst_size(3) // allow burst of 3 requests
        .key_extractor(SmartIpKeyExtractor)
        .finish()
        .unwrap();

    // Rate limiting: relaxed for lightweight API endpoints
    let api_governor = GovernorConfigBuilder::default()
        .per_second(2) // replenish 1 token every 2 seconds
        .burst_size(10) // allow burst of 10 requests
        .key_extractor(SmartIpKeyExtractor)
        .finish()
        .unwrap();

    // Background cleanup for rate limiter storage
    let gen_limiter = generate_governor.limiter().clone();
    let api_limiter = api_governor.limiter().clone();
    tokio::spawn(async move {
        let interval = std::time::Duration::from_secs(60);
        loop {
            tokio::time::sleep(interval).await;
            gen_limiter.retain_recent();
            api_limiter.retain_recent();
        }
    });

    // Expensive generation endpoints — strict rate limit only
    let generate_routes = Router::new()
        .route("/api/yomitan-dict", get(generate_dict))
        .route("/api/generate-stream", get(generate_stream))
        .layer(GovernorLayer {
            config: std::sync::Arc::new(generate_governor),
        });

    // Lightweight API endpoints — relaxed rate limit only
    let api_routes = Router::new()
        .route("/api/user-lists", get(fetch_user_lists))
        .route("/api/download", get(download_zip))
        .route("/api/yomitan-index", get(generate_index))
        .route("/api/build-info", get(build_info))
        .layer(GovernorLayer {
            config: std::sync::Arc::new(api_governor),
        });

    let app = Router::new()
        .route("/", get(serve_index))
        .route("/custom", get(serve_custom))
        .route("/custom/", get(serve_custom))
        .route("/api/health", get(health_check))
        .merge(generate_routes)
        .merge(api_routes)
        .nest_service("/static", ServeDir::new(static_dir()))
        .with_state(state);

    let port = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse::<u16>().ok())
        .unwrap_or(3000);
    let host = std::env::var("HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
    let addr = format!("{}:{}", host, port);
    let listener = match tokio::net::TcpListener::bind(&addr).await {
        Ok(listener) => listener,
        Err(error) => {
            error!(
                boot_id = %boot_id,
                address = %addr,
                error = %error,
                "Failed to bind TCP listener"
            );
            return;
        }
    };

    info!(
        boot_id = %boot_id,
        address = %addr,
        "Server listening"
    );

    let server = axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal(boot_id.clone()));

    if let Err(error) = server.await {
        error!(
            boot_id = %boot_id,
            error = %error,
            "Server exited with error"
        );
        return;
    }

    info!(boot_id = %boot_id, "Server shutdown complete");
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

async fn serve_custom() -> impl IntoResponse {
    let path = static_dir().join("custom").join("index.html");
    match tokio::fs::read(&path).await {
        Ok(bytes) => (
            StatusCode::OK,
            [("content-type", "text/html; charset=utf-8")],
            bytes,
        )
            .into_response(),
        Err(_) => (StatusCode::NOT_FOUND, "custom/index.html not found").into_response(),
    }
}

async fn build_info() -> impl IntoResponse {
    let timestamp = env!("BUILD_TIMESTAMP");
    axum::Json(serde_json::json!({ "build_time": timestamp }))
}

async fn health_check(State(state): State<AppState>) -> impl IntoResponse {
    let uptime = state.started_at.elapsed();
    let uptime_secs = uptime.as_secs();
    let hours = uptime_secs / 3600;
    let minutes = (uptime_secs % 3600) / 60;
    let seconds = uptime_secs % 60;

    let image_cache_bytes = state.image_cache.total_bytes();
    let image_cache_entries = state.image_cache.entry_count().await;

    let media_cache = state.media_cache.clone();
    let (media_cache_bytes, media_cache_entries) =
        tokio::task::spawn_blocking(move || (media_cache.total_bytes(), media_cache.entry_count()))
            .await
            .unwrap_or((0, 0));

    axum::Json(serde_json::json!({
        "status": "ok",
        "boot_id": state.boot_id,
        "uptime": format!("{}h {}m {}s", hours, minutes, seconds),
        "uptime_seconds": uptime_secs,
        "cache": {
            "image": {
                "entries": image_cache_entries,
                "size_bytes": image_cache_bytes,
                "size_mb": format!("{:.1}", image_cache_bytes as f64 / (1024.0 * 1024.0)),
            },
            "media": {
                "entries": media_cache_entries,
                "size_bytes": media_cache_bytes,
                "size_mb": format!("{:.1}", media_cache_bytes as f64 / (1024.0 * 1024.0)),
            }
        }
    }))
}

// === Fetch user lists endpoint ===

async fn fetch_user_lists(
    Query(params): Query<UserListQuery>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let vndb_user = normalize_vndb_user_input(params.vndb_user.as_deref().unwrap_or(""));
    let anilist_user = normalize_anilist_user_input(params.anilist_user.as_deref().unwrap_or(""));

    if vndb_user.is_empty() && anilist_user.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            [
                ("content-type", "application/json"),
                ("access-control-allow-origin", "*"),
            ],
            r#"{"error":"At least one username (vndb_user or anilist_user) is required"}"#
                .to_string(),
        )
            .into_response();
    }

    let mut all_entries: Vec<UserMediaEntry> = Vec::new();
    let mut raw_errors: Vec<String> = Vec::new();
    let mut response_errors: Vec<String> = Vec::new();

    if !vndb_user.is_empty() {
        let client = VndbClient::with_client(state.http_client.clone());
        match client.fetch_user_playing_list(&vndb_user).await {
            Ok(entries) => all_entries.extend(entries),
            Err(e) => {
                warn!(
                    boot_id = %state.boot_id,
                    source = "vndb",
                    user = %vndb_user,
                    error = %e,
                    "User list fetch failed"
                );
                response_errors.push(format!("VNDB: {}", public_error_message(&e)));
                raw_errors.push(e);
            }
        }
    }

    if !anilist_user.is_empty() {
        let client = AnilistClient::with_client(state.http_client.clone());
        match client.fetch_user_current_list(&anilist_user).await {
            Ok(entries) => all_entries.extend(entries),
            Err(e) => {
                warn!(
                    boot_id = %state.boot_id,
                    source = "anilist",
                    user = %anilist_user,
                    error = %e,
                    "User list fetch failed"
                );
                response_errors.push(format!("AniList: {}", public_error_message(&e)));
                raw_errors.push(e);
            }
        }
    }

    if all_entries.is_empty() && !raw_errors.is_empty() {
        let status = aggregate_status_code(&raw_errors);
        let error_msg = response_errors.join("; ");
        return (
            status,
            [
                ("content-type", "application/json"),
                ("access-control-allow-origin", "*"),
            ],
            serde_json::json!({"error": error_msg}).to_string(),
        )
            .into_response();
    }

    let response = serde_json::json!({
        "entries": all_entries,
        "errors": response_errors,
        "count": all_entries.len()
    });

    (
        StatusCode::OK,
        [
            ("content-type", "application/json"),
            ("access-control-allow-origin", "*"),
        ],
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
    let settings = params.to_settings();
    let vndb_user = normalize_vndb_user_input(&params.vndb_user.unwrap_or_default());
    let anilist_user = normalize_anilist_user_input(&params.anilist_user.unwrap_or_default());

    tokio::spawn(async move {
        let result =
            generate_dict_from_usernames(&vndb_user, &anilist_user, settings, Some(&tx), &state)
                .await;

        match result {
            Ok(zip_bytes) => {
                let token = uuid::Uuid::new_v4().to_string();
                {
                    let mut store = state.downloads.lock().await;
                    let now = std::time::Instant::now();
                    store.retain(|_, (_, created)| {
                        now.duration_since(*created).as_secs() < DOWNLOAD_TOKEN_MAX_AGE_SECS
                    });
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
                        .data(serde_json::json!({
                            "error": public_error_message(&e),
                            "status": status_code_for_error(&e).as_u16()
                        })
                        .to_string())))
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
                (
                    "content-disposition",
                    "attachment; filename=bee_characters.zip",
                ),
                ("access-control-allow-origin", "*"),
            ],
            zip_bytes,
        )
            .into_response()
    } else {
        (StatusCode::NOT_FOUND, "Download token not found or expired").into_response()
    }
}

/// Download and resize a single character image.
/// Checks the on-disk cache first; on miss, downloads, resizes, and caches.
/// Returns (resized_bytes, extension, width, height) or None on failure.
async fn fetch_image(
    url: &str,
    http_client: &reqwest::Client,
    image_cache: &ImageCache,
) -> Option<(Vec<u8>, String, u32, u32)> {
    // Check cache first
    if let Some((data, ext)) = image_cache.get(url).await {
        // Decode dimensions from cached bytes
        let (w, h) = image::load_from_memory(&data)
            .map(|img| (img.width(), img.height()))
            .unwrap_or((0, 0));
        return Some((data, ext, w, h));
    }

    let download_future = async {
        let response = http_client.get(url).send().await.ok()?;
        if response.status() != 200 {
            warn!(url = url, status = %response.status(), "Image download returned non-200");
            return None;
        }
        response.bytes().await.ok()
    };

    let raw_bytes =
        match tokio::time::timeout(std::time::Duration::from_secs(10), download_future).await {
            Ok(Some(bytes)) => bytes,
            Ok(None) => return None,
            Err(_) => {
                warn!(url = url, "Image download timed out after 10s");
                return None;
            }
        };

    // Resize to thumbnail + convert to JPEG
    let (resized, ext, w, h) = ImageHandler::resize_image(&raw_bytes);

    // Write to cache (fire-and-forget, non-blocking)
    image_cache.put(url, &resized, ext).await;

    Some((resized, ext.to_string(), w, h))
}

/// Download images for all characters concurrently, with resize.
/// Concurrency is capped to respect API rate limits.
async fn download_images_concurrent(
    char_data: &mut models::CharacterData,
    http_client: &reqwest::Client,
    image_cache: &ImageCache,
    concurrency: usize,
) {
    // Collect (index_in_flat_list, url) pairs
    let all_chars: Vec<_> = char_data.all_characters().enumerate().collect();
    let urls: Vec<(usize, String)> = all_chars
        .iter()
        .filter_map(|(i, c)| c.image_url.as_ref().map(|url| (*i, url.clone())))
        .collect();

    // Download concurrently
    let results: Vec<IndexedImageResult> = stream::iter(urls)
        .map(|(idx, url)| {
            let client = http_client.clone();
            let cache = image_cache.clone();
            async move {
                let result = fetch_image(&url, &client, &cache).await;
                (idx, result)
            }
        })
        .buffer_unordered(concurrency)
        .collect()
        .await;

    // Apply results back to characters
    let mut flat: Vec<&mut models::Character> = char_data.all_characters_mut().collect();
    for (idx, result) in results {
        if let Some((bytes, ext, w, h)) = result {
            if let Some(ch) = flat.get_mut(idx) {
                ch.image_bytes = Some(bytes);
                ch.image_ext = Some(ext);
                if w > 0 && h > 0 {
                    ch.image_width = Some(w);
                    ch.image_height = Some(h);
                }
            }
        }
    }
}

/// Download seiyuu (voice actor) images for all characters that have a seiyuu_image_url.
/// Uses the same cache and resize pipeline as character images.
async fn download_seiyuu_images(
    char_data: &mut models::CharacterData,
    http_client: &reqwest::Client,
    image_cache: &ImageCache,
    concurrency: usize,
) {
    let all_chars: Vec<_> = char_data.all_characters().enumerate().collect();
    let urls: Vec<(usize, String)> = all_chars
        .iter()
        .filter_map(|(i, c)| c.seiyuu_image_url.as_ref().map(|url| (*i, url.clone())))
        .collect();

    if urls.is_empty() {
        return;
    }

    let results: Vec<IndexedImageResult> = stream::iter(urls)
        .map(|(idx, url)| {
            let client = http_client.clone();
            let cache = image_cache.clone();
            async move {
                let result = fetch_image(&url, &client, &cache).await;
                (idx, result)
            }
        })
        .buffer_unordered(concurrency)
        .collect()
        .await;

    let mut flat: Vec<&mut models::Character> = char_data.all_characters_mut().collect();
    for (idx, result) in results {
        if let Some((bytes, ext, w, h)) = result {
            if let Some(ch) = flat.get_mut(idx) {
                ch.seiyuu_image_bytes = Some(bytes);
                ch.seiyuu_image_ext = Some(ext);
                if w > 0 && h > 0 {
                    ch.seiyuu_image_width = Some(w);
                    ch.seiyuu_image_height = Some(h);
                }
            }
        }
    }
}

// === Core function: Generate dictionary from usernames ===

async fn generate_dict_from_usernames(
    vndb_user: &str,
    anilist_user: &str,
    settings: DictSettings,
    progress_tx: Option<&tokio::sync::mpsc::Sender<Result<Event, std::convert::Infallible>>>,
    state: &AppState,
) -> Result<Vec<u8>, String> {
    let started_at = std::time::Instant::now();
    let vndb_user = normalize_vndb_user_input(vndb_user);
    let anilist_user = normalize_anilist_user_input(anilist_user);
    let mut stats = GenerationStats::default();
    let mut collected_errors: Vec<String> = Vec::new();

    // Step 1: Collect all media entries from user lists
    let mut media_entries: Vec<UserMediaEntry> = Vec::new();

    if !vndb_user.is_empty() {
        let client = VndbClient::with_client(state.http_client.clone());
        match client.fetch_user_playing_list(&vndb_user).await {
            Ok(entries) => media_entries.extend(entries),
            Err(e) => {
                stats.record_failure(&e);
                collected_errors.push(e.clone());
                if anilist_user.is_empty() {
                    log_generation_summary(
                        state,
                        "usernames",
                        started_at,
                        &stats,
                        None,
                        None,
                        Some(&e),
                    );
                    return Err(e);
                }
                warn!(user = %vndb_user, error = %e, "VNDB list fetch error (continuing)");
            }
        }
    }

    if !anilist_user.is_empty() {
        let client = AnilistClient::with_client(state.http_client.clone());
        match client.fetch_user_current_list(&anilist_user).await {
            Ok(entries) => media_entries.extend(entries),
            Err(e) => {
                stats.record_failure(&e);
                collected_errors.push(e.clone());
                if vndb_user.is_empty() || media_entries.is_empty() {
                    log_generation_summary(
                        state,
                        "usernames",
                        started_at,
                        &stats,
                        None,
                        None,
                        Some(&e),
                    );
                    return Err(e);
                }
                warn!(
                    user = %anilist_user,
                    error = %e,
                    "AniList list fetch error (continuing)"
                );
            }
        }
    }

    // Deduplicate media entries by (source, id) to avoid processing the same
    // title twice (e.g. if the API returns duplicates or the same manga/VN
    // appears multiple times in a user list).
    {
        let mut seen = HashSet::new();
        media_entries.retain(|entry| seen.insert((entry.source.clone(), entry.id.clone())));
    }

    if media_entries.is_empty() {
        let error = if collected_errors.is_empty() {
            format!(
                "{} No in-progress media found in the requested user lists",
                INVALID_INPUT_PREFIX
            )
        } else {
            combine_service_errors(&collected_errors)
        };
        log_generation_summary(
            state,
            "usernames",
            started_at,
            &stats,
            None,
            None,
            Some(&error),
        );
        return Err(error);
    }

    let total = media_entries.len();
    stats.media_total = total;

    // Build download URL with usernames for auto-update (percent-encoded)
    let base = base_url();
    let mut url_parts = Vec::new();
    if !vndb_user.is_empty() {
        url_parts.push(format!("vndb_user={}", urlencoding::encode(&vndb_user)));
    }
    if !anilist_user.is_empty() {
        url_parts.push(format!(
            "anilist_user={}",
            urlencoding::encode(&anilist_user)
        ));
    }
    // Append non-default settings
    if !settings.honorifics {
        url_parts.push("honorifics=false".to_string());
    }
    if !settings.show_image {
        url_parts.push("image=false".to_string());
    }
    if !settings.show_tag {
        url_parts.push("tag=false".to_string());
    }
    if !settings.show_description {
        url_parts.push("description=false".to_string());
    }
    if !settings.show_traits {
        url_parts.push("traits=false".to_string());
    }
    if !settings.show_spoilers {
        url_parts.push("spoilers=false".to_string());
    }
    if !settings.show_seiyuu {
        url_parts.push("seiyuu=false".to_string());
    }
    let download_url = format!("{}/api/yomitan-dict?{}", base, url_parts.join("&"));

    let description = format!("Character Dictionary ({} titles)", total);

    let mut builder = DictBuilder::new(settings, Some(download_url), description);

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

        let _game_title = &entry.title;

        match entry.source.as_str() {
            "vndb" => {
                match fetch_vndb_cached(&entry.id, state).await {
                    Ok((title, mut char_data, cached)) => {
                        stats.record_vndb_fetch(cached);
                        download_images_concurrent(
                            &mut char_data,
                            &state.http_client,
                            &state.image_cache,
                            8,
                        )
                        .await;
                        download_seiyuu_images(
                            &mut char_data,
                            &state.http_client,
                            &state.image_cache,
                            4,
                        )
                        .await;

                        for character in char_data.all_characters() {
                            builder.add_character(character, &title);
                        }

                        // Only sleep on cache miss (API call was made, respect rate limit)
                        if !cached {
                            tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
                        }
                    }
                    Err(e) => {
                        stats.record_failure(&e);
                        collected_errors.push(e.clone());
                        warn!(vn_id = %entry.id, error = %e, "Failed to fetch VNDB characters");
                    }
                }
            }
            "anilist" => {
                let media_id: i32 = match parse_anilist_id(&entry.id) {
                    Ok(id) => id,
                    Err(_) => {
                        let error = format!(
                            "{} Invalid AniList media ID '{}'",
                            INVALID_INPUT_PREFIX, entry.id
                        );
                        stats.record_failure(&error);
                        collected_errors.push(error.clone());
                        warn!(id = %entry.id, "Invalid AniList media ID");
                        continue;
                    }
                };

                let media_type = match entry.media_type.as_str() {
                    "anime" => "ANIME",
                    "manga" => "MANGA",
                    _ => "ANIME",
                };

                match fetch_anilist_cached(media_id, media_type, state).await {
                    Ok((title, mut char_data, cached)) => {
                        stats.record_anilist_fetch(cached);
                        download_images_concurrent(
                            &mut char_data,
                            &state.http_client,
                            &state.image_cache,
                            6,
                        )
                        .await;
                        download_seiyuu_images(
                            &mut char_data,
                            &state.http_client,
                            &state.image_cache,
                            4,
                        )
                        .await;

                        for character in char_data.all_characters() {
                            builder.add_character(character, &title);
                        }

                        // Only sleep on cache miss (API call was made, respect rate limit)
                        if !cached {
                            tokio::time::sleep(tokio::time::Duration::from_millis(700)).await;
                        }
                    }
                    Err(e) => {
                        stats.record_failure(&e);
                        collected_errors.push(e.clone());
                        warn!(media_id = %entry.id, error = %e, "Failed to fetch AniList characters");
                    }
                }
            }
            _ => {
                warn!(source = %entry.source, "Unknown source");
            }
        }
    }

    builder.log_skipped_no_japanese_summary();

    if !builder.has_entries() {
        let error = if !collected_errors.is_empty() {
            combine_service_errors(&collected_errors)
        } else {
            format!(
                "{} No character entries were generated from the requested media",
                INVALID_INPUT_PREFIX
            )
        };
        log_generation_summary(
            state,
            "usernames",
            started_at,
            &stats,
            Some(&builder),
            None,
            Some(&error),
        );
        return Err(error);
    }

    let zip_bytes = match builder.export_bytes() {
        Ok(zip_bytes) => zip_bytes,
        Err(error) => {
            log_generation_summary(
                state,
                "usernames",
                started_at,
                &stats,
                Some(&builder),
                None,
                Some(&error),
            );
            return Err(error);
        }
    };

    log_generation_summary(
        state,
        "usernames",
        started_at,
        &stats,
        Some(&builder),
        Some(zip_bytes.len()),
        None,
    );

    Ok(zip_bytes)
}

// === Generate dictionary from multiple manual media entries ===

async fn generate_dict_from_entries(
    entries: &[ManualEntry],
    settings: DictSettings,
    state: &AppState,
) -> Result<Vec<u8>, String> {
    let started_at = std::time::Instant::now();
    let mut stats = GenerationStats::default();
    let mut collected_errors: Vec<String> = Vec::new();

    // Normalize + deduplicate entries by (source, id)
    let mut seen = HashSet::new();
    let mut unique_entries: Vec<NormalizedManualEntry> = Vec::new();
    for entry in entries {
        match normalize_manual_entry(entry) {
            Ok(entry) => {
                if seen.insert((entry.source.clone(), entry.id.clone())) {
                    unique_entries.push(entry);
                }
            }
            Err(error) => {
                stats.record_failure(&error);
                collected_errors.push(error.clone());
                warn!(
                    source = %entry.source,
                    id = %entry.id,
                    error = %error,
                    "Skipping invalid manual entry"
                );
            }
        }
    }

    if unique_entries.is_empty() {
        let error = if collected_errors.is_empty() {
            format!("{} No valid entries provided", INVALID_INPUT_PREFIX)
        } else {
            combine_service_errors(&collected_errors)
        };
        log_generation_summary(
            state,
            "manual_entries",
            started_at,
            &stats,
            None,
            None,
            Some(&error),
        );
        return Err(error);
    }

    let total = unique_entries.len();
    stats.media_total = total;

    // Build download URL with entries JSON for auto-update
    let base = base_url();
    let entries_json: Vec<serde_json::Value> = unique_entries
        .iter()
        .map(|e| {
            let mut obj = serde_json::json!({
                "source": e.source,
                "id": e.id,
            });
            if e.source == "anilist" {
                obj["media_type"] = serde_json::json!(e.media_type);
            }
            obj
        })
        .collect();
    let mut url_parts = vec![format!(
        "entries={}",
        urlencoding::encode(&serde_json::to_string(&entries_json).unwrap_or_default())
    )];
    if !settings.honorifics {
        url_parts.push("honorifics=false".to_string());
    }
    if !settings.show_image {
        url_parts.push("image=false".to_string());
    }
    if !settings.show_tag {
        url_parts.push("tag=false".to_string());
    }
    if !settings.show_description {
        url_parts.push("description=false".to_string());
    }
    if !settings.show_traits {
        url_parts.push("traits=false".to_string());
    }
    if !settings.show_spoilers {
        url_parts.push("spoilers=false".to_string());
    }
    if !settings.show_seiyuu {
        url_parts.push("seiyuu=false".to_string());
    }
    let download_url = format!("{}/api/yomitan-dict?{}", base, url_parts.join("&"));

    let description = format!("Character Dictionary ({} titles)", total);
    let mut builder = DictBuilder::new(settings, Some(download_url), description);

    for entry in unique_entries.iter() {
        match entry.source.as_str() {
            "vndb" => match fetch_vndb_cached(&entry.id, state).await {
                Ok((title, mut char_data, cached)) => {
                    stats.record_vndb_fetch(cached);
                    download_images_concurrent(
                        &mut char_data,
                        &state.http_client,
                        &state.image_cache,
                        8,
                    )
                    .await;
                    download_seiyuu_images(
                        &mut char_data,
                        &state.http_client,
                        &state.image_cache,
                        4,
                    )
                    .await;

                    for character in char_data.all_characters() {
                        builder.add_character(character, &title);
                    }

                    if !cached {
                        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
                    }
                }
                Err(e) => {
                    stats.record_failure(&e);
                    collected_errors.push(e.clone());
                    warn!(vn_id = %entry.id, error = %e, "Failed to fetch VNDB characters");
                }
            },
            "anilist" => {
                let media_id: i32 = match entry.id.parse() {
                    Ok(id) => id,
                    Err(_) => {
                        let error = format!(
                            "{} Invalid AniList media ID '{}'",
                            INVALID_INPUT_PREFIX, entry.id
                        );
                        stats.record_failure(&error);
                        collected_errors.push(error.clone());
                        warn!(id = %entry.id, error = %error, "Invalid AniList media ID");
                        continue;
                    }
                };

                let media_type = entry.media_type.as_str();

                match fetch_anilist_cached(media_id, media_type, state).await {
                    Ok((title, mut char_data, cached)) => {
                        stats.record_anilist_fetch(cached);
                        download_images_concurrent(
                            &mut char_data,
                            &state.http_client,
                            &state.image_cache,
                            6,
                        )
                        .await;
                        download_seiyuu_images(
                            &mut char_data,
                            &state.http_client,
                            &state.image_cache,
                            4,
                        )
                        .await;

                        for character in char_data.all_characters() {
                            builder.add_character(character, &title);
                        }

                        if !cached {
                            tokio::time::sleep(tokio::time::Duration::from_millis(700)).await;
                        }
                    }
                    Err(e) => {
                        stats.record_failure(&e);
                        collected_errors.push(e.clone());
                        warn!(media_id = %entry.id, error = %e, "Failed to fetch AniList characters");
                    }
                }
            }
            _ => {
                warn!(source = %entry.source, "Unknown source in entries");
            }
        }
    }

    builder.log_skipped_no_japanese_summary();

    if !builder.has_entries() {
        let error = if !collected_errors.is_empty() {
            combine_service_errors(&collected_errors)
        } else {
            format!(
                "{} No character entries were generated from the requested media",
                INVALID_INPUT_PREFIX
            )
        };
        log_generation_summary(
            state,
            "manual_entries",
            started_at,
            &stats,
            Some(&builder),
            None,
            Some(&error),
        );
        return Err(error);
    }

    let zip_bytes = match builder.export_bytes() {
        Ok(zip_bytes) => zip_bytes,
        Err(error) => {
            log_generation_summary(
                state,
                "manual_entries",
                started_at,
                &stats,
                Some(&builder),
                None,
                Some(&error),
            );
            return Err(error);
        }
    };

    log_generation_summary(
        state,
        "manual_entries",
        started_at,
        &stats,
        Some(&builder),
        Some(zip_bytes.len()),
        None,
    );
    Ok(zip_bytes)
}

// === Generate dictionary (single media OR username-based OR multi-entry) ===

async fn generate_dict(
    Query(params): Query<DictQuery>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let settings = params.to_settings();

    let vndb_user = normalize_vndb_user_input(params.vndb_user.as_deref().unwrap_or(""));
    let anilist_user = normalize_anilist_user_input(params.anilist_user.as_deref().unwrap_or(""));

    if !vndb_user.is_empty() || !anilist_user.is_empty() {
        match generate_dict_from_usernames(&vndb_user, &anilist_user, settings, None, &state).await
        {
            Ok(bytes) => {
                return (
                    StatusCode::OK,
                    [
                        ("content-type", "application/zip"),
                        (
                            "content-disposition",
                            "attachment; filename=bee_characters.zip",
                        ),
                        ("access-control-allow-origin", "*"),
                    ],
                    bytes,
                )
                    .into_response();
            }
            Err(e) => {
                warn!(
                    boot_id = %state.boot_id,
                    vndb_user = %vndb_user,
                    anilist_user = %anilist_user,
                    error = %e,
                    "Dictionary generation request failed"
                );
                return (status_code_for_error(&e), public_error_message(&e)).into_response();
            }
        }
    }

    // Multi-entry mode: entries=JSON
    if let Some(ref entries_json) = params.entries {
        let manual_entries: Vec<ManualEntry> = match serde_json::from_str(entries_json) {
            Ok(e) => e,
            Err(e) => {
                return (
                    StatusCode::BAD_REQUEST,
                    format!("Invalid entries JSON: {}", e),
                )
                    .into_response();
            }
        };

        if manual_entries.is_empty() {
            return (StatusCode::BAD_REQUEST, "entries array is empty").into_response();
        }

        let settings = params.to_settings();
        match generate_dict_from_entries(&manual_entries, settings, &state).await {
            Ok(bytes) => {
                return (
                    StatusCode::OK,
                    [
                        ("content-type", "application/zip"),
                        (
                            "content-disposition",
                            "attachment; filename=bee_characters.zip",
                        ),
                        ("access-control-allow-origin", "*"),
                    ],
                    bytes,
                )
                    .into_response();
            }
            Err(e) => {
                warn!(
                    boot_id = %state.boot_id,
                    error = %e,
                    "Manual dictionary generation request failed"
                );
                return (status_code_for_error(&e), public_error_message(&e)).into_response();
            }
        }
    }

    // Single-media mode
    let source = params.source.as_deref().unwrap_or("");
    let id = params.id.as_deref().unwrap_or("");

    if source.is_empty() || id.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            "Either provide source+id, entries JSON, or vndb_user/anilist_user",
        )
            .into_response();
    }

    let result = match source.to_lowercase().as_str() {
        "vndb" => {
            let normalized_id = match VndbClient::parse_vn_id(id) {
                Ok(id) => id,
                Err(e) => return (StatusCode::BAD_REQUEST, e).into_response(),
            };
            let download_url = {
                let base = base_url();
                let mut parts = vec![
                    format!("source={}", urlencoding::encode(source)),
                    format!("id={}", urlencoding::encode(&normalized_id)),
                    format!("media_type={}", urlencoding::encode(&params.media_type)),
                ];
                params.append_settings_params(&mut parts);
                format!("{}/api/yomitan-dict?{}", base, parts.join("&"))
            };
            generate_vndb_dict(&normalized_id, settings, &download_url, &state).await
        }
        "anilist" => {
            let media_id: i32 = match parse_anilist_id(id) {
                Ok(id) => id,
                Err(e) => return (StatusCode::BAD_REQUEST, e).into_response(),
            };
            let media_type = params.media_type.to_uppercase();
            if media_type != "ANIME" && media_type != "MANGA" {
                return (StatusCode::BAD_REQUEST, "media_type must be ANIME or MANGA")
                    .into_response();
            }
            let download_url = {
                let base = base_url();
                let mut parts = vec![
                    format!("source={}", urlencoding::encode(source)),
                    format!("id={}", urlencoding::encode(&media_id.to_string())),
                    format!("media_type={}", urlencoding::encode(&media_type)),
                ];
                params.append_settings_params(&mut parts);
                format!("{}/api/yomitan-dict?{}", base, parts.join("&"))
            };
            generate_anilist_dict(media_id, &media_type, settings, &download_url, &state).await
        }
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                "source must be 'vndb' or 'anilist'",
            )
                .into_response()
        }
    };

    match result {
        Ok(bytes) => (
            StatusCode::OK,
            [
                ("content-type", "application/zip"),
                (
                    "content-disposition",
                    "attachment; filename=bee_characters.zip",
                ),
                ("access-control-allow-origin", "*"),
            ],
            bytes,
        )
            .into_response(),
        Err(e) => {
            warn!(
                boot_id = %state.boot_id,
                source = %source,
                id = %id,
                error = %e,
                "Single-media dictionary generation request failed"
            );
            (status_code_for_error(&e), public_error_message(&e)).into_response()
        }
    }
}

/// Lightweight endpoint: returns just the index.json metadata as JSON.
async fn generate_index(Query(params): Query<DictQuery>) -> impl IntoResponse {
    let vndb_user = normalize_vndb_user_input(params.vndb_user.as_deref().unwrap_or(""));
    let anilist_user = normalize_anilist_user_input(params.anilist_user.as_deref().unwrap_or(""));

    let download_url = if !vndb_user.is_empty() || !anilist_user.is_empty() {
        let base = base_url();
        let mut url_parts = Vec::new();
        if !vndb_user.is_empty() {
            url_parts.push(format!("vndb_user={}", urlencoding::encode(&vndb_user)));
        }
        if !anilist_user.is_empty() {
            url_parts.push(format!(
                "anilist_user={}",
                urlencoding::encode(&anilist_user)
            ));
        }
        params.append_settings_params(&mut url_parts);
        format!("{}/api/yomitan-dict?{}", base, url_parts.join("&"))
    } else if let Some(ref entries_json) = params.entries {
        // Multi-entry mode: pass entries JSON through to download URL
        let base = base_url();
        let normalized_entries_json = normalize_entries_json_for_url(entries_json);
        let mut parts = vec![format!(
            "entries={}",
            urlencoding::encode(&normalized_entries_json)
        )];
        params.append_settings_params(&mut parts);
        format!("{}/api/yomitan-dict?{}", base, parts.join("&"))
    } else {
        let base = base_url();
        let source = params.source.as_deref().unwrap_or("");
        let id = match source.to_lowercase().as_str() {
            "vndb" => normalize_vndb_id_for_url(params.id.as_deref().unwrap_or("")),
            "anilist" => normalize_anilist_id_for_url(params.id.as_deref().unwrap_or("")),
            _ => params.id.as_deref().unwrap_or("").trim().to_string(),
        };
        let mut parts = vec![
            format!("source={}", urlencoding::encode(source)),
            format!("id={}", urlencoding::encode(&id)),
            format!("media_type={}", urlencoding::encode(&params.media_type)),
        ];
        params.append_settings_params(&mut parts);
        format!("{}/api/yomitan-dict?{}", base, parts.join("&"))
    };

    let settings = params.to_settings();
    let builder = DictBuilder::new(settings, Some(download_url), String::new());
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

// === Cached fetch wrappers ===

/// Fetch VNDB character data, checking the media cache first.
///
/// Returns `(title, char_data, cached)` where `cached` is true on cache hit.
/// On cache miss, fetches from the VNDB API and stores the result.
/// Image bytes are always `None` in the returned data — call
/// `download_images_concurrent()` afterward.
async fn fetch_vndb_cached(
    vn_id: &str,
    state: &AppState,
) -> Result<(String, models::CharacterData, bool), String> {
    let vn_id = VndbClient::parse_vn_id(vn_id).map_err(|e| format!("{} {}", INVALID_INPUT_PREFIX, e))?;
    let cache_key = format!("vndb:{}", vn_id);

    // Check cache first (blocking SQLite read, but fast).
    let cache = state.media_cache.clone();
    let key_clone = cache_key.clone();
    let cached = tokio::task::spawn_blocking(move || cache.get(&key_clone))
        .await
        .map_err(|e| format!("Cache read failed: {}", e))?;

    if let Some(entry) = cached {
        return Ok((entry.title, entry.char_data, true));
    }

    // Cache miss — fetch from API.
    let client = VndbClient::with_client(state.http_client.clone());

    let vn_info = client
        .fetch_vn_info(&vn_id)
        .await
        .unwrap_or_else(|_| vndb_client::VnInfo {
            title: "Unknown VN".to_string(),
            alttitle: String::new(),
            va_map: std::collections::HashMap::new(),
        });
    let title = if !vn_info.alttitle.is_empty() {
        vn_info.alttitle
    } else {
        vn_info.title
    };

    let mut char_data = client.fetch_characters(&vn_id).await?;

    // Apply voice actor data from VN endpoint to characters
    for c in char_data.all_characters_mut() {
        if let Some(va_name) = vn_info.va_map.get(&c.id) {
            c.seiyuu = Some(va_name.clone());
        }
    }

    // Clear image bytes before caching (images handled by ImageCache).
    for c in char_data.all_characters_mut() {
        c.image_bytes = None;
        c.image_ext = None;
        c.seiyuu_image_bytes = None;
        c.seiyuu_image_ext = None;
    }

    // Store in cache (blocking SQLite write).
    let cache = state.media_cache.clone();
    let key_clone = cache_key;
    let title_clone = title.clone();
    let data_clone = char_data.clone();
    tokio::task::spawn_blocking(move || cache.put(&key_clone, &title_clone, &data_clone))
        .await
        .map_err(|e| format!("Cache write failed: {}", e))?;

    Ok((title, char_data, false))
}

/// Fetch AniList character data, checking the media cache first.
///
/// Returns `(title, char_data, cached)` where `cached` is true on cache hit.
/// On cache miss, fetches from the AniList API and stores the result.
/// Image bytes are always `None` in the returned data — call
/// `download_images_concurrent()` afterward.
async fn fetch_anilist_cached(
    media_id: i32,
    media_type: &str,
    state: &AppState,
) -> Result<(String, models::CharacterData, bool), String> {
    let cache_key = format!("anilist:{}:{}", media_id, media_type);

    // Check cache first.
    let cache = state.media_cache.clone();
    let key_clone = cache_key.clone();
    let cached = tokio::task::spawn_blocking(move || cache.get(&key_clone))
        .await
        .map_err(|e| format!("Cache read failed: {}", e))?;

    if let Some(entry) = cached {
        return Ok((entry.title, entry.char_data, true));
    }

    // Cache miss — fetch from API.
    let client = AnilistClient::with_client(state.http_client.clone());
    let (mut char_data, media_title) = client.fetch_characters(media_id, media_type).await?;

    let title = if !media_title.is_empty() {
        media_title
    } else {
        format!("AniList {}", media_id)
    };

    // Clear image bytes before caching.
    for c in char_data.all_characters_mut() {
        c.image_bytes = None;
        c.image_ext = None;
        c.seiyuu_image_bytes = None;
        c.seiyuu_image_ext = None;
    }

    // Store in cache.
    let cache = state.media_cache.clone();
    let key_clone = cache_key;
    let title_clone = title.clone();
    let data_clone = char_data.clone();
    tokio::task::spawn_blocking(move || cache.put(&key_clone, &title_clone, &data_clone))
        .await
        .map_err(|e| format!("Cache write failed: {}", e))?;

    Ok((title, char_data, false))
}

// === Single-media helpers ===

async fn generate_vndb_dict(
    vn_id: &str,
    settings: DictSettings,
    download_url: &str,
    state: &AppState,
) -> Result<Vec<u8>, String> {
    let started_at = std::time::Instant::now();
    let mut stats = GenerationStats {
        media_total: 1,
        ..GenerationStats::default()
    };
    let (game_title, mut char_data, cached) = match fetch_vndb_cached(vn_id, state).await {
        Ok(result) => result,
        Err(error) => {
            stats.record_failure(&error);
            log_generation_summary(
                state,
                "single_vndb",
                started_at,
                &stats,
                None,
                None,
                Some(&error),
            );
            return Err(error);
        }
    };
    stats.record_vndb_fetch(cached);

    // Concurrent image downloads with resize
    download_images_concurrent(&mut char_data, &state.http_client, &state.image_cache, 8).await;
    download_seiyuu_images(&mut char_data, &state.http_client, &state.image_cache, 4).await;

    let mut builder =
        DictBuilder::new(settings, Some(download_url.to_string()), game_title.clone());

    for character in char_data.all_characters() {
        builder.add_character(character, &game_title);
    }

    builder.log_skipped_no_japanese_summary();

    if !builder.has_entries() {
        let error = format!(
            "{} No character entries were generated for the requested VNDB media",
            INVALID_INPUT_PREFIX
        );
        log_generation_summary(
            state,
            "single_vndb",
            started_at,
            &stats,
            Some(&builder),
            None,
            Some(&error),
        );
        return Err(error);
    }

    let zip_bytes = match builder.export_bytes() {
        Ok(zip_bytes) => zip_bytes,
        Err(error) => {
            log_generation_summary(
                state,
                "single_vndb",
                started_at,
                &stats,
                Some(&builder),
                None,
                Some(&error),
            );
            return Err(error);
        }
    };

    log_generation_summary(
        state,
        "single_vndb",
        started_at,
        &stats,
        Some(&builder),
        Some(zip_bytes.len()),
        None,
    );

    Ok(zip_bytes)
}

async fn generate_anilist_dict(
    media_id: i32,
    media_type: &str,
    settings: DictSettings,
    download_url: &str,
    state: &AppState,
) -> Result<Vec<u8>, String> {
    let started_at = std::time::Instant::now();
    let mut stats = GenerationStats {
        media_total: 1,
        ..GenerationStats::default()
    };
    let (game_title, mut char_data, cached) =
        match fetch_anilist_cached(media_id, media_type, state).await {
            Ok(result) => result,
            Err(error) => {
                stats.record_failure(&error);
                log_generation_summary(
                    state,
                    "single_anilist",
                    started_at,
                    &stats,
                    None,
                    None,
                    Some(&error),
                );
                return Err(error);
            }
        };
    stats.record_anilist_fetch(cached);

    // Concurrent image downloads with resize
    download_images_concurrent(&mut char_data, &state.http_client, &state.image_cache, 6).await;
    download_seiyuu_images(&mut char_data, &state.http_client, &state.image_cache, 4).await;

    let mut builder =
        DictBuilder::new(settings, Some(download_url.to_string()), game_title.clone());

    for character in char_data.all_characters() {
        builder.add_character(character, &game_title);
    }

    builder.log_skipped_no_japanese_summary();

    if !builder.has_entries() {
        let error = format!(
            "{} No character entries were generated for the requested AniList media",
            INVALID_INPUT_PREFIX
        );
        log_generation_summary(
            state,
            "single_anilist",
            started_at,
            &stats,
            Some(&builder),
            None,
            Some(&error),
        );
        return Err(error);
    }

    let zip_bytes = match builder.export_bytes() {
        Ok(zip_bytes) => zip_bytes,
        Err(error) => {
            log_generation_summary(
                state,
                "single_anilist",
                started_at,
                &stats,
                Some(&builder),
                None,
                Some(&error),
            );
            return Err(error);
        }
    };

    log_generation_summary(
        state,
        "single_anilist",
        started_at,
        &stats,
        Some(&builder),
        Some(zip_bytes.len()),
        None,
    );

    Ok(zip_bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_status_code_for_invalid_input_error() {
        assert_eq!(
            status_code_for_error("INVALID_INPUT: bad request"),
            StatusCode::BAD_REQUEST
        );
    }

    #[test]
    fn test_status_code_for_rate_limit_error() {
        assert_eq!(
            status_code_for_error("RATE_LIMIT: upstream busy"),
            StatusCode::SERVICE_UNAVAILABLE
        );
    }

    #[test]
    fn test_status_code_for_upstream_error() {
        assert_eq!(
            status_code_for_error("UPSTREAM: request failed"),
            StatusCode::BAD_GATEWAY
        );
    }

    #[test]
    fn test_parse_anilist_id_plain_number() {
        assert_eq!(parse_anilist_id("9253").unwrap(), 9253);
    }

    #[test]
    fn test_parse_anilist_id_with_whitespace() {
        assert_eq!(parse_anilist_id("  9253  ").unwrap(), 9253);
    }

    #[test]
    fn test_parse_anilist_id_anime_url() {
        assert_eq!(
            parse_anilist_id("https://anilist.co/anime/9253").unwrap(),
            9253
        );
    }

    #[test]
    fn test_parse_anilist_id_manga_url() {
        assert_eq!(
            parse_anilist_id("https://anilist.co/manga/30002").unwrap(),
            30002
        );
    }

    #[test]
    fn test_parse_anilist_id_url_with_slug() {
        assert_eq!(
            parse_anilist_id("https://anilist.co/anime/9253/Steins-Gate").unwrap(),
            9253
        );
    }

    #[test]
    fn test_parse_anilist_id_url_with_query() {
        assert_eq!(
            parse_anilist_id("https://anilist.co/anime/9253?tab=characters").unwrap(),
            9253
        );
    }

    #[test]
    fn test_parse_anilist_id_url_with_fragment() {
        assert_eq!(
            parse_anilist_id("https://anilist.co/anime/9253#top").unwrap(),
            9253
        );
    }

    #[test]
    fn test_parse_anilist_id_http_url() {
        assert_eq!(
            parse_anilist_id("http://anilist.co/anime/9253").unwrap(),
            9253
        );
    }

    #[test]
    fn test_parse_anilist_id_bare_domain() {
        assert_eq!(parse_anilist_id("anilist.co/anime/9253").unwrap(), 9253);
    }

    #[test]
    fn test_parse_anilist_id_url_with_whitespace() {
        assert_eq!(
            parse_anilist_id("  https://anilist.co/anime/9253  ").unwrap(),
            9253
        );
    }

    #[test]
    fn test_parse_anilist_id_invalid_string() {
        assert!(parse_anilist_id("abc").is_err());
    }

    #[test]
    fn test_parse_anilist_id_empty() {
        assert!(parse_anilist_id("").is_err());
    }

    #[test]
    fn test_parse_anilist_id_url_missing_id_segment() {
        assert!(parse_anilist_id("https://anilist.co/anime/").is_err());
    }

    #[test]
    fn test_parse_anilist_id_url_non_numeric_id() {
        assert!(parse_anilist_id("https://anilist.co/anime/abc").is_err());
    }

    #[test]
    fn test_media_entries_dedup_same_source_and_id() {
        use crate::models::UserMediaEntry;

        let mut entries = vec![
            UserMediaEntry {
                id: "v17".to_string(),
                title: "Steins;Gate".to_string(),
                title_romaji: "Steins;Gate".to_string(),
                source: "vndb".to_string(),
                media_type: "vn".to_string(),
            },
            UserMediaEntry {
                id: "v17".to_string(),
                title: "Steins;Gate".to_string(),
                title_romaji: "Steins;Gate".to_string(),
                source: "vndb".to_string(),
                media_type: "vn".to_string(),
            },
            UserMediaEntry {
                id: "9253".to_string(),
                title: "Steins;Gate".to_string(),
                title_romaji: "Steins;Gate".to_string(),
                source: "anilist".to_string(),
                media_type: "anime".to_string(),
            },
        ];

        let mut seen = HashSet::new();
        entries.retain(|entry| seen.insert((entry.source.clone(), entry.id.clone())));

        assert_eq!(entries.len(), 2, "Duplicate VNDB entry should be removed");
        assert_eq!(entries[0].source, "vndb");
        assert_eq!(entries[1].source, "anilist");
    }

    #[test]
    fn test_media_entries_dedup_same_id_different_source() {
        use crate::models::UserMediaEntry;

        let mut entries = vec![
            UserMediaEntry {
                id: "9253".to_string(),
                title: "Steins;Gate".to_string(),
                title_romaji: "Steins;Gate".to_string(),
                source: "vndb".to_string(),
                media_type: "vn".to_string(),
            },
            UserMediaEntry {
                id: "9253".to_string(),
                title: "Steins;Gate".to_string(),
                title_romaji: "Steins;Gate".to_string(),
                source: "anilist".to_string(),
                media_type: "anime".to_string(),
            },
        ];

        let mut seen = HashSet::new();
        entries.retain(|entry| seen.insert((entry.source.clone(), entry.id.clone())));

        assert_eq!(
            entries.len(),
            2,
            "Same ID from different sources should both be kept"
        );
    }

    #[test]
    fn test_media_entries_dedup_preserves_first_occurrence() {
        use crate::models::UserMediaEntry;

        let mut entries = vec![
            UserMediaEntry {
                id: "30002".to_string(),
                title: "First Title".to_string(),
                title_romaji: "First".to_string(),
                source: "anilist".to_string(),
                media_type: "manga".to_string(),
            },
            UserMediaEntry {
                id: "30002".to_string(),
                title: "Second Title".to_string(),
                title_romaji: "Second".to_string(),
                source: "anilist".to_string(),
                media_type: "manga".to_string(),
            },
        ];

        let mut seen = HashSet::new();
        entries.retain(|entry| seen.insert((entry.source.clone(), entry.id.clone())));

        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].title, "First Title",
            "Should keep the first occurrence"
        );
    }

    // ===== Additional comprehensive tests =====

    // --- parse_anilist_id edge cases ---

    #[test]
    fn test_parse_anilist_id_negative_number() {
        // Negative numbers are valid i32 values, so they parse successfully
        assert_eq!(parse_anilist_id("-1").unwrap(), -1);
    }

    #[test]
    fn test_parse_anilist_id_zero() {
        assert_eq!(parse_anilist_id("0").unwrap(), 0);
    }

    #[test]
    fn test_parse_anilist_id_very_large() {
        assert_eq!(parse_anilist_id("2147483647").unwrap(), 2147483647); // i32::MAX
    }

    #[test]
    fn test_parse_anilist_id_overflow() {
        assert!(parse_anilist_id("2147483648").is_err()); // i32::MAX + 1
    }

    #[test]
    fn test_parse_anilist_id_float() {
        assert!(parse_anilist_id("9253.5").is_err());
    }

    #[test]
    fn test_parse_anilist_id_url_with_www() {
        // www.anilist.co still contains "anilist.co/" so it parses successfully
        assert_eq!(
            parse_anilist_id("https://www.anilist.co/anime/9253").unwrap(),
            9253
        );
    }

    #[test]
    fn test_parse_anilist_id_url_multiple_slashes() {
        assert_eq!(
            parse_anilist_id("https://anilist.co/anime/9253/Steins-Gate/characters").unwrap(),
            9253
        );
    }

    #[test]
    fn test_parse_anilist_id_url_with_both_query_and_fragment() {
        assert_eq!(
            parse_anilist_id("https://anilist.co/anime/9253?tab=chars#top").unwrap(),
            9253
        );
    }

    // --- Media entry deduplication edge cases ---

    #[test]
    fn test_media_entries_dedup_empty_list() {
        let mut entries: Vec<UserMediaEntry> = vec![];
        let mut seen = HashSet::new();
        entries.retain(|entry| seen.insert((entry.source.clone(), entry.id.clone())));
        assert!(entries.is_empty());
    }

    #[test]
    fn test_media_entries_dedup_single_entry() {
        let mut entries = vec![UserMediaEntry {
            id: "v17".to_string(),
            title: "Test".to_string(),
            title_romaji: "Test".to_string(),
            source: "vndb".to_string(),
            media_type: "vn".to_string(),
        }];
        let mut seen = HashSet::new();
        entries.retain(|entry| seen.insert((entry.source.clone(), entry.id.clone())));
        assert_eq!(entries.len(), 1);
    }

    #[test]
    fn test_media_entries_dedup_many_duplicates() {
        let mut entries: Vec<UserMediaEntry> = (0..10)
            .map(|_| UserMediaEntry {
                id: "v17".to_string(),
                title: "Test".to_string(),
                title_romaji: "Test".to_string(),
                source: "vndb".to_string(),
                media_type: "vn".to_string(),
            })
            .collect();
        let mut seen = HashSet::new();
        entries.retain(|entry| seen.insert((entry.source.clone(), entry.id.clone())));
        assert_eq!(entries.len(), 1);
    }

    #[test]
    fn test_media_entries_dedup_mixed_sources() {
        let mut entries = vec![
            UserMediaEntry {
                id: "1".to_string(),
                title: "A".to_string(),
                title_romaji: "A".to_string(),
                source: "vndb".to_string(),
                media_type: "vn".to_string(),
            },
            UserMediaEntry {
                id: "1".to_string(),
                title: "A".to_string(),
                title_romaji: "A".to_string(),
                source: "anilist".to_string(),
                media_type: "anime".to_string(),
            },
            UserMediaEntry {
                id: "2".to_string(),
                title: "B".to_string(),
                title_romaji: "B".to_string(),
                source: "vndb".to_string(),
                media_type: "vn".to_string(),
            },
            UserMediaEntry {
                id: "2".to_string(),
                title: "B".to_string(),
                title_romaji: "B".to_string(),
                source: "anilist".to_string(),
                media_type: "manga".to_string(),
            },
            UserMediaEntry {
                id: "1".to_string(),
                title: "A dup".to_string(),
                title_romaji: "A".to_string(),
                source: "vndb".to_string(),
                media_type: "vn".to_string(),
            },
        ];
        let mut seen = HashSet::new();
        entries.retain(|entry| seen.insert((entry.source.clone(), entry.id.clone())));
        assert_eq!(entries.len(), 4); // 4 unique (source, id) pairs
    }

    // --- base_url tests ---

    #[test]
    fn test_base_url_default() {
        // When no env vars are set, should default to http://127.0.0.1:3000
        // (Can't easily test this without modifying env, but we can test the function exists)
        let url = base_url();
        assert!(url.starts_with("http"));
    }

    // ===================================================================
    // DictQuery → DictSettings conversion tests
    // ===================================================================

    /// Helper: build a DictQuery with all defaults.
    fn make_dict_query_default() -> DictQuery {
        DictQuery {
            source: None,
            id: None,
            entries: None,
            media_type: default_media_type(),
            vndb_user: None,
            anilist_user: None,
            honorifics: true,
            image: true,
            tag: true,
            description: true,
            traits: true,
            spoilers: true,
            seiyuu: true,
        }
    }

    // ===================================================================
    // GenerateStreamQuery → DictSettings conversion tests
    // ===================================================================

    fn make_stream_query_default() -> GenerateStreamQuery {
        GenerateStreamQuery {
            vndb_user: None,
            anilist_user: None,
            honorifics: true,
            image: true,
            tag: true,
            description: true,
            traits: true,
            spoilers: true,
            seiyuu: true,
        }
    }

    #[test]
    fn test_stream_query_defaults_all_true() {
        let q = make_stream_query_default();
        let s = q.to_settings();
        assert!(s.show_image);
        assert!(s.show_tag);
        assert!(s.show_description);
        assert!(s.show_traits);
        assert!(s.show_spoilers);
        assert!(s.honorifics);
    }

    #[test]
    fn test_stream_query_all_false() {
        let q = GenerateStreamQuery {
            honorifics: false,
            image: false,
            tag: false,
            description: false,
            traits: false,
            spoilers: false,
            ..make_stream_query_default()
        };
        let s = q.to_settings();
        assert!(!s.honorifics);
        assert!(!s.show_image);
        assert!(!s.show_tag);
        assert!(!s.show_description);
        assert!(!s.show_traits);
        assert!(!s.show_spoilers);
    }

    #[test]
    fn test_stream_query_mixed() {
        let q = GenerateStreamQuery {
            description: false,
            traits: false,
            ..make_stream_query_default()
        };
        let s = q.to_settings();
        assert!(!s.show_description);
        assert!(!s.show_traits);
        assert!(s.show_image);
        assert!(s.show_tag);
        assert!(s.show_spoilers);
        assert!(s.honorifics);
    }

    #[test]
    fn test_stream_query_with_usernames() {
        let q = GenerateStreamQuery {
            vndb_user: Some("foo".to_string()),
            anilist_user: Some("bar".to_string()),
            image: false,
            ..make_stream_query_default()
        };
        assert_eq!(q.vndb_user.as_deref(), Some("foo"));
        assert_eq!(q.anilist_user.as_deref(), Some("bar"));
        let s = q.to_settings();
        assert!(!s.show_image);
    }

    // ===================================================================
    // DictQuery and GenerateStreamQuery produce identical DictSettings
    // ===================================================================

    #[test]
    fn test_dict_and_stream_query_produce_same_settings() {
        let dict_q = DictQuery {
            honorifics: false,
            image: false,
            tag: true,
            description: false,
            traits: true,
            spoilers: false,
            ..make_dict_query_default()
        };
        let stream_q = GenerateStreamQuery {
            honorifics: false,
            image: false,
            tag: true,
            description: false,
            traits: true,
            spoilers: false,
            ..make_stream_query_default()
        };
        let ds = dict_q.to_settings();
        let ss = stream_q.to_settings();

        assert_eq!(ds.honorifics, ss.honorifics);
        assert_eq!(ds.show_image, ss.show_image);
        assert_eq!(ds.show_tag, ss.show_tag);
        assert_eq!(ds.show_description, ss.show_description);
        assert_eq!(ds.show_traits, ss.show_traits);
        assert_eq!(ds.show_spoilers, ss.show_spoilers);
    }

    // ===================================================================
    // URL round-trip: settings survive through append_settings_params
    // ===================================================================

    #[test]
    fn test_settings_url_roundtrip() {
        let q1 = DictQuery {
            source: Some("vndb".to_string()),
            id: Some("v17".to_string()),
            honorifics: false,
            spoilers: false,
            image: false,
            ..make_dict_query_default()
        };
        let s1 = q1.to_settings();
        assert!(!s1.honorifics);
        assert!(!s1.show_spoilers);
        assert!(!s1.show_image);
        assert!(s1.show_tag);
        assert!(s1.show_description);
        assert!(s1.show_traits);

        // Reconstruct URL via append_settings_params
        let mut parts = vec![
            format!("source={}", q1.source.as_deref().unwrap()),
            format!("id={}", q1.id.as_deref().unwrap()),
        ];
        q1.append_settings_params(&mut parts);

        // Verify the right params were added
        assert!(parts.contains(&"honorifics=false".to_string()));
        assert!(parts.contains(&"image=false".to_string()));
        assert!(parts.contains(&"spoilers=false".to_string()));
        // tag, description, traits are default=true, should not be added
        assert!(!parts.iter().any(|p| p.starts_with("tag=")));
        assert!(!parts.iter().any(|p| p.starts_with("description=")));
        assert!(!parts.iter().any(|p| p.starts_with("traits=")));
    }

    #[test]
    fn test_settings_url_roundtrip_all_defaults() {
        let q1 = DictQuery {
            source: Some("anilist".to_string()),
            id: Some("9253".to_string()),
            ..make_dict_query_default()
        };
        let mut parts = vec!["source=anilist".to_string(), "id=9253".to_string()];
        q1.append_settings_params(&mut parts);
        // No extra params since all are default
        assert_eq!(parts.len(), 2);
    }

    #[test]
    fn test_settings_url_roundtrip_all_false() {
        let q1 = DictQuery {
            honorifics: false,
            image: false,
            tag: false,
            description: false,
            traits: false,
            spoilers: false,
            ..make_dict_query_default()
        };

        let mut parts = Vec::new();
        q1.append_settings_params(&mut parts);
        assert_eq!(parts.len(), 6, "All-false should produce 6 params");

        // Verify every setting is represented
        let joined = parts.join("&");
        assert!(joined.contains("honorifics=false"));
        assert!(joined.contains("image=false"));
        assert!(joined.contains("tag=false"));
        assert!(joined.contains("description=false"));
        assert!(joined.contains("traits=false"));
        assert!(joined.contains("spoilers=false"));
    }

    // ===================================================================
    // to_settings mapping correctness
    // ===================================================================

    #[test]
    fn test_to_settings_field_mapping_dict_query() {
        // Verify each DictQuery field maps to the correct DictSettings field
        let q = DictQuery {
            image: false,
            tag: true,
            description: false,
            traits: true,
            spoilers: false,
            honorifics: true,
            ..make_dict_query_default()
        };
        let s = q.to_settings();
        assert_eq!(s.show_image, q.image);
        assert_eq!(s.show_tag, q.tag);
        assert_eq!(s.show_description, q.description);
        assert_eq!(s.show_traits, q.traits);
        assert_eq!(s.show_spoilers, q.spoilers);
        assert_eq!(s.honorifics, q.honorifics);
    }

    #[test]
    fn test_to_settings_field_mapping_stream_query() {
        let q = GenerateStreamQuery {
            image: false,
            tag: true,
            description: true,
            traits: false,
            spoilers: true,
            honorifics: false,
            ..make_stream_query_default()
        };
        let s = q.to_settings();
        assert_eq!(s.show_image, q.image);
        assert_eq!(s.show_tag, q.tag);
        assert_eq!(s.show_description, q.description);
        assert_eq!(s.show_traits, q.traits);
        assert_eq!(s.show_spoilers, q.spoilers);
        assert_eq!(s.honorifics, q.honorifics);
    }

    // ===================================================================
    // Each setting independently toggleable
    // ===================================================================

    #[test]
    fn test_each_setting_independently_toggleable() {
        let fields = [
            "honorifics",
            "image",
            "tag",
            "description",
            "traits",
            "spoilers",
        ];
        for field in fields {
            let q = DictQuery {
                honorifics: field != "honorifics",
                image: field != "image",
                tag: field != "tag",
                description: field != "description",
                traits: field != "traits",
                spoilers: field != "spoilers",
                ..make_dict_query_default()
            };
            let s = q.to_settings();
            // The one field that was set to false should be false, all others true
            match field {
                "honorifics" => {
                    assert!(!s.honorifics);
                    assert!(
                        s.show_image
                            && s.show_tag
                            && s.show_description
                            && s.show_traits
                            && s.show_spoilers
                    );
                }
                "image" => {
                    assert!(!s.show_image);
                    assert!(
                        s.honorifics
                            && s.show_tag
                            && s.show_description
                            && s.show_traits
                            && s.show_spoilers
                    );
                }
                "tag" => {
                    assert!(!s.show_tag);
                    assert!(
                        s.honorifics
                            && s.show_image
                            && s.show_description
                            && s.show_traits
                            && s.show_spoilers
                    );
                }
                "description" => {
                    assert!(!s.show_description);
                    assert!(
                        s.honorifics
                            && s.show_image
                            && s.show_tag
                            && s.show_traits
                            && s.show_spoilers
                    );
                }
                "traits" => {
                    assert!(!s.show_traits);
                    assert!(
                        s.honorifics
                            && s.show_image
                            && s.show_tag
                            && s.show_description
                            && s.show_spoilers
                    );
                }
                "spoilers" => {
                    assert!(!s.show_spoilers);
                    assert!(
                        s.honorifics
                            && s.show_image
                            && s.show_tag
                            && s.show_description
                            && s.show_traits
                    );
                }
                _ => unreachable!(),
            }
        }
    }
}
