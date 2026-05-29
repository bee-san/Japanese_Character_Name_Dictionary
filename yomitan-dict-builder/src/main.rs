use axum::{
    extract::{Query, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
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
use tower_http::{compression::CompressionLayer, services::ServeDir};
use tracing::{debug, error, info, warn};

mod anilist_client;
mod content_builder;
mod dict_builder;
mod frequency_dict_builder;
mod image_cache;
mod image_handler;
mod jiten_client;
mod kana;
mod media_cache;
mod models;
mod name_parser;
mod vndb_client;

#[cfg(test)]
mod anilist_name_test_data;

use anilist_client::{AnilistClient, AnilistRetryPolicy, AnilistShelfStatus};
use content_builder::DictSettings;
use dict_builder::DictBuilder;
use frequency_dict_builder::{
    FrequencyCombineMode, FrequencyDictBuilder, FrequencyDisplayMode, FREQUENCY_DICTIONARY_TITLE,
};
use image_cache::ImageCache;
use image_handler::ImageHandler;
use jiten_client::JitenClient;
use media_cache::MediaCache;
use models::UserMediaEntry;
use vndb_client::{VndbClient, VndbShelfStatus};

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

/// Shared application state for temporary generated dictionary storage.
type DownloadStore = Arc<Mutex<HashMap<String, DownloadArtifact>>>;

#[derive(Clone)]
struct DownloadArtifact {
    bytes: Vec<u8>,
    filename: String,
    content_type: &'static str,
    created_at: std::time::Instant,
}

impl DownloadArtifact {
    fn character_zip(bytes: Vec<u8>) -> Self {
        Self {
            bytes,
            filename: "bee_characters.zip".to_string(),
            content_type: "application/zip",
            created_at: std::time::Instant::now(),
        }
    }

    fn frequency_zip(bytes: Vec<u8>) -> Self {
        Self {
            bytes,
            filename: "bee_frequency.zip".to_string(),
            content_type: "application/zip",
            created_at: std::time::Instant::now(),
        }
    }
}

/// Result of fetching an image: the index into the character list, and optionally the image bytes + extension.
type IndexedImageResult = (usize, Option<(Vec<u8>, String, u32, u32)>);

/// Interval for cleaning up expired download tokens.
const DOWNLOAD_CLEANUP_INTERVAL_SECS: u64 = 60;

/// Max age for download tokens (5 minutes).
const DOWNLOAD_TOKEN_MAX_AGE_SECS: u64 = 300;
const INVALID_INPUT_PREFIX: &str = "INVALID_INPUT:";
const RATE_LIMIT_PREFIX: &str = "RATE_LIMIT:";
const UPSTREAM_PREFIX: &str = "UPSTREAM:";
const DEFAULT_LOG_FILTER: &str = "info,tower_governor=warn";
const MULTI_MEDIA_ABORT_MESSAGE_PREFIX: &str = "Dictionary generation aborted because";
const ANILIST_MEDIA_SEARCH_MAX_RESULTS: usize = 8;
const VNDB_MEDIA_SEARCH_MAX_RESULTS: usize = 8;

#[derive(Clone)]
struct AppState {
    downloads: DownloadStore,
    /// Shared HTTP client for preview/general API calls and image downloads.
    http_client: reqwest::Client,
    /// Dedicated AniList client for generation cache misses with a longer timeout budget.
    anilist_generation_http_client: reqwest::Client,
    /// On-disk image cache with popularity-based eviction.
    image_cache: ImageCache,
    /// Per-media API response cache (character data + title).
    media_cache: MediaCache,
    /// Jiten API client for media frequency decks.
    jiten_client: JitenClient,
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
                    store.retain(|_, artifact| {
                        now.duration_since(artifact.created_at).as_secs()
                            < DOWNLOAD_TOKEN_MAX_AGE_SECS
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
            .timeout(AnilistRetryPolicy::Preview.request_timeout())
            .build()
            .unwrap_or_else(|e| {
                error!(
                    boot_id = %boot_id,
                    timeout_secs = AnilistRetryPolicy::Preview.request_timeout().as_secs(),
                    error = %e,
                    "Preview/general HTTP client initialization failed"
                );
                std::process::exit(1)
            });
        let anilist_generation_http_client = reqwest::Client::builder()
            .timeout(AnilistRetryPolicy::Generation.request_timeout())
            .build()
            .unwrap_or_else(|e| {
                error!(
                    boot_id = %boot_id,
                    timeout_secs = AnilistRetryPolicy::Generation.request_timeout().as_secs(),
                    error = %e,
                    "AniList generation HTTP client initialization failed"
                );
                std::process::exit(1)
            });

        let jiten_client = JitenClient::new(http_client.clone());

        Self {
            downloads,
            http_client,
            anilist_generation_http_client,
            image_cache,
            media_cache,
            jiten_client,
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct MediaGenerationFailure {
    source: String,
    id: String,
    title: String,
    error: String,
}

impl MediaGenerationFailure {
    fn new(source: &str, id: &str, title: &str, error: &str) -> Self {
        Self {
            source: source.to_string(),
            id: id.to_string(),
            title: title.to_string(),
            error: error.to_string(),
        }
    }

    fn redacted_preview_label(&self) -> String {
        // Media IDs and titles can reveal a user's private watch/play list when
        // generation is driven from VNDB/AniList usernames. Keep only the source
        // dimension in aggregate logs; per-request details remain in the
        // user-facing error path without writing personal list contents to logs.
        format!("{}:<redacted>", self.source)
    }
}

#[derive(Debug, Clone, serde::Serialize)]
struct FrequencyUnmatchedMedia {
    #[serde(flatten)]
    media: UserMediaEntry,
    reason: String,
}

struct FrequencyGenerationResult {
    zip_bytes: Vec<u8>,
    matched_count: usize,
    unmatched: Vec<FrequencyUnmatchedMedia>,
    total_terms: usize,
}

#[derive(Clone)]
struct NormalizedManualEntry {
    source: String,
    id: String,
    media_type: String,
    status: String,
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
    vndb_status: Option<String>, // comma-separated VNDB shelf statuses
    anilist_status: Option<String>, // comma-separated AniList shelf statuses
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
    #[serde(default = "default_manual_status")]
    status: String,
}

#[derive(Deserialize)]
struct UserListQuery {
    vndb_user: Option<String>,
    anilist_user: Option<String>,
    vndb_status: Option<String>,
    anilist_status: Option<String>,
}

#[derive(Deserialize)]
struct AnilistMediaSearchQuery {
    q: Option<String>,
    media_type: Option<String>,
}

#[derive(Deserialize)]
struct VndbMediaSearchQuery {
    q: Option<String>,
}

#[derive(Deserialize)]
struct GenerateStreamQuery {
    vndb_user: Option<String>,
    anilist_user: Option<String>,
    vndb_status: Option<String>,
    anilist_status: Option<String>,
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

#[derive(Deserialize, Clone, Default)]
struct FrequencyQuery {
    vndb_user: Option<String>,
    anilist_user: Option<String>,
    vndb_status: Option<String>,
    anilist_status: Option<String>,
    entries: Option<String>,
    min_occurrences: Option<u64>,
    max_terms: Option<usize>,
    #[serde(default)]
    display_mode: FrequencyDisplayMode,
    #[serde(default)]
    combine_mode: FrequencyCombineMode,
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

fn default_manual_status() -> String {
    "current".to_string()
}

fn default_true() -> bool {
    true
}

fn normalize_manual_status(status: &str) -> String {
    match status.trim().to_ascii_lowercase().as_str() {
        "playing" => "playing".to_string(),
        "finished" => "finished".to_string(),
        "wishlist" => "wishlist".to_string(),
        "completed" => "completed".to_string(),
        "planning" => "planning".to_string(),
        "paused" => "paused".to_string(),
        "dropped" => "dropped".to_string(),
        _ => "current".to_string(),
    }
}

fn parse_shelf_status_params(
    vndb_status: Option<&str>,
    anilist_status: Option<&str>,
) -> Result<(Vec<VndbShelfStatus>, Vec<AnilistShelfStatus>), String> {
    let vndb_statuses = VndbShelfStatus::parse_list(vndb_status)
        .map_err(|e| format!("{} {}", INVALID_INPUT_PREFIX, e))?;
    let anilist_statuses = AnilistShelfStatus::parse_list(anilist_status)
        .map_err(|e| format!("{} {}", INVALID_INPUT_PREFIX, e))?;
    Ok((vndb_statuses, anilist_statuses))
}

fn status_query_value<T>(statuses: &[T], query_value: impl Fn(T) -> &'static str) -> String
where
    T: Copy,
{
    statuses
        .iter()
        .copied()
        .map(query_value)
        .collect::<Vec<_>>()
        .join(",")
}

fn append_shelf_status_params(
    parts: &mut Vec<String>,
    include_vndb: bool,
    vndb_statuses: &[VndbShelfStatus],
    include_anilist: bool,
    anilist_statuses: &[AnilistShelfStatus],
) {
    if include_vndb && !VndbShelfStatus::is_default_list(vndb_statuses) {
        let value = status_query_value(vndb_statuses, VndbShelfStatus::query_value);
        parts.push(format!("vndb_status={}", urlencoding::encode(&value)));
    }
    if include_anilist && !AnilistShelfStatus::is_default_list(anilist_statuses) {
        let value = status_query_value(anilist_statuses, AnilistShelfStatus::query_value);
        parts.push(format!("anilist_status={}", urlencoding::encode(&value)));
    }
}

fn normalize_anilist_media_search_query(
    params: &AnilistMediaSearchQuery,
) -> Result<(String, String), String> {
    let query = params.q.as_deref().unwrap_or("").trim().to_string();
    if query.chars().filter(|ch| !ch.is_whitespace()).count() < 2 {
        return Err(format!(
            "{} q must contain at least 2 non-space characters",
            INVALID_INPUT_PREFIX
        ));
    }

    let media_type = params
        .media_type
        .as_deref()
        .unwrap_or("ANIME")
        .trim()
        .to_uppercase();
    match media_type.as_str() {
        "ANIME" | "MANGA" => Ok((query, media_type)),
        _ => Err(format!(
            "{} media_type must be ANIME or MANGA",
            INVALID_INPUT_PREFIX
        )),
    }
}

fn normalize_vndb_media_search_query(params: &VndbMediaSearchQuery) -> Result<String, String> {
    let query = params.q.as_deref().unwrap_or("").trim().to_string();
    if query.chars().filter(|ch| !ch.is_whitespace()).count() < 2 {
        return Err(format!(
            "{} q must contain at least 2 non-space characters",
            INVALID_INPUT_PREFIX
        ));
    }

    Ok(query)
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
    let stripped = strip_service_error_prefix(error);
    if stripped.starts_with(MULTI_MEDIA_ABORT_MESSAGE_PREFIX) {
        return stripped.to_string();
    }

    match classify_service_error_kind(error) {
        ServiceErrorKind::InvalidInput => stripped.to_string(),
        ServiceErrorKind::RateLimit => {
            "Upstream service is temporarily rate limiting requests. Please try again shortly."
                .to_string()
        }
        ServiceErrorKind::Upstream => {
            "Failed to fetch data from an upstream service. Please try again shortly.".to_string()
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

fn new_request_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

fn failed_media_preview(media_failures: &[MediaGenerationFailure]) -> Vec<String> {
    media_failures
        .iter()
        .take(5)
        .map(MediaGenerationFailure::redacted_preview_label)
        .collect()
}

fn generation_summary_failure_context(
    request_id: &str,
    media_failures: &[MediaGenerationFailure],
) -> (String, usize, Vec<String>) {
    (
        request_id.to_string(),
        media_failures.len(),
        failed_media_preview(media_failures),
    )
}

fn build_multi_media_abort_error(
    total_requested: usize,
    media_failures: &[MediaGenerationFailure],
    extra_errors: &[String],
) -> String {
    let aggregated_errors: Vec<String> = media_failures
        .iter()
        .map(|failure| failure.error.clone())
        .chain(extra_errors.iter().cloned())
        .collect();

    let prefix = match aggregate_status_code(&aggregated_errors) {
        StatusCode::BAD_REQUEST => INVALID_INPUT_PREFIX,
        StatusCode::SERVICE_UNAVAILABLE => RATE_LIMIT_PREFIX,
        StatusCode::BAD_GATEWAY => UPSTREAM_PREFIX,
        _ => "",
    };
    let message = format!(
        "Dictionary generation aborted because {} of {} media failed.",
        media_failures.len(),
        total_requested
    );

    if prefix.is_empty() {
        message
    } else {
        format!("{} {}", prefix, message)
    }
}

fn normalize_vndb_user_input(input: &str) -> String {
    VndbClient::normalize_user_input(input)
}

fn normalize_anilist_user_input(input: &str) -> String {
    AnilistClient::normalize_user_input(input)
}

fn anilist_preview_client(state: &AppState) -> AnilistClient {
    AnilistClient::with_client(state.http_client.clone())
}

fn anilist_generation_client(state: &AppState) -> AnilistClient {
    AnilistClient::with_retry_policy(
        state.anilist_generation_http_client.clone(),
        AnilistRetryPolicy::Generation,
    )
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
    let status = normalize_manual_status(&entry.status);
    match source.as_str() {
        "vndb" => Ok(NormalizedManualEntry {
            source,
            id: VndbClient::parse_vn_id(&entry.id)
                .map_err(|e| format!("{} {}", INVALID_INPUT_PREFIX, e))?,
            media_type: "vn".to_string(),
            status,
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
            status,
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
            if entry.status != "current" {
                value["status"] = serde_json::json!(entry.status);
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

struct GenerationSummaryContext<'a> {
    state: &'a AppState,
    request_id: &'a str,
    mode: &'a str,
    started_at: std::time::Instant,
    stats: &'a GenerationStats,
    media_failures: &'a [MediaGenerationFailure],
}

impl<'a> GenerationSummaryContext<'a> {
    fn new(
        state: &'a AppState,
        request_id: &'a str,
        mode: &'a str,
        started_at: std::time::Instant,
        stats: &'a GenerationStats,
        media_failures: &'a [MediaGenerationFailure],
    ) -> Self {
        Self {
            state,
            request_id,
            mode,
            started_at,
            stats,
            media_failures,
        }
    }

    fn log(
        &self,
        builder: Option<&DictBuilder>,
        zip_size_bytes: Option<usize>,
        error: Option<&str>,
    ) {
        let skipped_no_japanese_count = builder
            .map(DictBuilder::skipped_no_japanese_count)
            .unwrap_or(0);
        let duration_ms = self.started_at.elapsed().as_millis() as u64;
        let zip_size_bytes = zip_size_bytes.unwrap_or(0);
        let (request_id, failed_media_count, failed_media_preview) =
            generation_summary_failure_context(self.request_id, self.media_failures);

        match error {
            Some(error) => warn!(
                boot_id = %self.state.boot_id,
                request_id = %request_id,
                mode = self.mode,
                media_total = self.stats.media_total,
                cache_hits = self.stats.total_cache_hits(),
                cache_misses = self.stats.total_cache_misses(),
                vndb_cache_hits = self.stats.vndb_cache_hits,
                vndb_cache_misses = self.stats.vndb_cache_misses,
                anilist_cache_hits = self.stats.anilist_cache_hits,
                anilist_cache_misses = self.stats.anilist_cache_misses,
                upstream_failures = self.stats.total_external_failures(),
                rate_limit_failures = self.stats.rate_limit_failures,
                invalid_input_failures = self.stats.invalid_input_failures,
                skipped_no_japanese_count = skipped_no_japanese_count,
                failed_media_count = failed_media_count,
                failed_media_preview = ?failed_media_preview,
                zip_size_bytes = zip_size_bytes,
                duration_ms = duration_ms,
                error = %error,
                "Dictionary generation failed"
            ),
            None => info!(
                boot_id = %self.state.boot_id,
                request_id = %request_id,
                mode = self.mode,
                media_total = self.stats.media_total,
                cache_hits = self.stats.total_cache_hits(),
                cache_misses = self.stats.total_cache_misses(),
                vndb_cache_hits = self.stats.vndb_cache_hits,
                vndb_cache_misses = self.stats.vndb_cache_misses,
                anilist_cache_hits = self.stats.anilist_cache_hits,
                anilist_cache_misses = self.stats.anilist_cache_misses,
                upstream_failures = self.stats.total_external_failures(),
                rate_limit_failures = self.stats.rate_limit_failures,
                invalid_input_failures = self.stats.invalid_input_failures,
                skipped_no_japanese_count = skipped_no_japanese_count,
                failed_media_count = failed_media_count,
                failed_media_preview = ?failed_media_preview,
                zip_size_bytes = zip_size_bytes,
                duration_ms = duration_ms,
                "Dictionary generation completed"
            ),
        }
    }
}

fn finalize_multi_media_generation(
    builder: &mut DictBuilder,
    total_requested: usize,
    collected_errors: &[String],
    log_context: &GenerationSummaryContext<'_>,
) -> Result<Vec<u8>, String> {
    builder.log_skipped_no_japanese_summary();

    if !log_context.media_failures.is_empty() {
        let error = build_multi_media_abort_error(
            total_requested,
            log_context.media_failures,
            collected_errors,
        );
        log_context.log(Some(builder), None, Some(&error));
        return Err(error);
    }

    if !collected_errors.is_empty() {
        let error = combine_service_errors(collected_errors);
        log_context.log(Some(builder), None, Some(&error));
        return Err(error);
    }

    if !builder.has_entries() {
        let error = format!(
            "{} No character entries were generated from the requested media",
            INVALID_INPUT_PREFIX
        );
        log_context.log(Some(builder), None, Some(&error));
        return Err(error);
    }

    let zip_bytes = match builder.export_bytes() {
        Ok(zip_bytes) => zip_bytes,
        Err(error) => {
            log_context.log(Some(builder), None, Some(&error));
            return Err(error);
        }
    };

    log_context.log(Some(builder), Some(zip_bytes.len()), None);

    Ok(zip_bytes)
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
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(DEFAULT_LOG_FILTER)),
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
        .route("/api/yomitan-frequency-dict", get(generate_frequency_dict))
        .route(
            "/api/generate-frequency-stream",
            get(generate_frequency_stream),
        )
        .layer(GovernorLayer {
            config: std::sync::Arc::new(generate_governor),
        });

    // Lightweight API endpoints — relaxed rate limit only
    let api_routes = Router::new()
        .route("/api/user-lists", get(fetch_user_lists))
        .route("/api/anilist-media-search", get(anilist_media_search))
        .route("/api/vndb-media-search", get(vndb_media_search))
        .route("/api/download", get(download_zip))
        .route("/api/yomitan-index", get(generate_index))
        .route(
            "/api/yomitan-frequency-index",
            get(generate_frequency_index),
        )
        .route("/api/build-info", get(build_info))
        .layer(GovernorLayer {
            config: std::sync::Arc::new(api_governor),
        });

    let app = Router::new()
        .route("/", get(serve_index))
        .route("/custom", get(serve_custom))
        .route("/custom/", get(serve_custom))
        .route("/frequency", get(serve_frequency))
        .route("/frequency/", get(serve_frequency))
        .route("/api/health", get(health_check))
        .merge(generate_routes)
        .merge(api_routes)
        .nest_service("/static", ServeDir::new(static_dir()))
        .layer(CompressionLayer::new())
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

async fn serve_frequency() -> impl IntoResponse {
    let path = static_dir().join("frequency").join("index.html");
    match tokio::fs::read(&path).await {
        Ok(bytes) => (
            StatusCode::OK,
            [("content-type", "text/html; charset=utf-8")],
            bytes,
        )
            .into_response(),
        Err(_) => (StatusCode::NOT_FOUND, "frequency/index.html not found").into_response(),
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

async fn collect_user_media_entries(
    state: &AppState,
    vndb_user: Option<&str>,
    anilist_user: Option<&str>,
    vndb_statuses: &[VndbShelfStatus],
    anilist_statuses: &[AnilistShelfStatus],
) -> Result<(Vec<UserMediaEntry>, Vec<String>), String> {
    let vndb_user = normalize_vndb_user_input(vndb_user.unwrap_or(""));
    let anilist_user = normalize_anilist_user_input(anilist_user.unwrap_or(""));

    if vndb_user.is_empty() && anilist_user.is_empty() {
        return Err(format!(
            "{} At least one username (vndb_user or anilist_user) is required",
            INVALID_INPUT_PREFIX
        ));
    }

    let mut all_entries = Vec::new();
    let mut raw_errors = Vec::new();
    let mut response_warnings = Vec::new();

    if !vndb_user.is_empty() {
        let client = VndbClient::with_client(state.http_client.clone());
        match client.fetch_user_list(&vndb_user, vndb_statuses).await {
            Ok(entries) => all_entries.extend(entries),
            Err(error) => {
                response_warnings.push(format!("VNDB: {}", public_error_message(&error)));
                raw_errors.push(error);
            }
        }
    }

    if !anilist_user.is_empty() {
        let client = anilist_preview_client(state);
        match client
            .fetch_user_list(&anilist_user, anilist_statuses)
            .await
        {
            Ok(entries) => all_entries.extend(entries),
            Err(error) => {
                response_warnings.push(format!("AniList: {}", public_error_message(&error)));
                raw_errors.push(error);
            }
        }
    }

    {
        let mut seen = HashSet::new();
        all_entries.retain(|entry| seen.insert((entry.source.clone(), entry.id.clone())));
    }

    if all_entries.is_empty() {
        if raw_errors.is_empty() {
            Err(format!(
                "{} No selected media found in the requested user lists",
                INVALID_INPUT_PREFIX
            ))
        } else {
            Err(combine_service_errors(&raw_errors))
        }
    } else {
        Ok((all_entries, response_warnings))
    }
}

// === Fetch user lists endpoint ===

async fn fetch_user_lists(
    Query(params): Query<UserListQuery>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let request_id = new_request_id();
    let (vndb_statuses, anilist_statuses) = match parse_shelf_status_params(
        params.vndb_status.as_deref(),
        params.anilist_status.as_deref(),
    ) {
        Ok(statuses) => statuses,
        Err(error) => {
            return (
                status_code_for_error(&error),
                [
                    ("content-type", "application/json"),
                    ("access-control-allow-origin", "*"),
                ],
                serde_json::json!({"error": public_error_message(&error)}).to_string(),
            )
                .into_response();
        }
    };
    let (all_entries, response_errors) = match collect_user_media_entries(
        &state,
        params.vndb_user.as_deref(),
        params.anilist_user.as_deref(),
        &vndb_statuses,
        &anilist_statuses,
    )
    .await
    {
        Ok(result) => result,
        Err(error) => {
            warn!(
                boot_id = %state.boot_id,
                request_id = %request_id,
                error = %error,
                "User list fetch failed"
            );
            return (
                status_code_for_error(&error),
                [
                    ("content-type", "application/json"),
                    ("access-control-allow-origin", "*"),
                ],
                serde_json::json!({"error": public_error_message(&error)}).to_string(),
            )
                .into_response();
        }
    };

    let response = serde_json::json!({
        "entries": all_entries,
        "errors": response_errors,
        "count": all_entries.len()
    });

    debug!(
        boot_id = %state.boot_id,
        request_id = %request_id,
        entries = all_entries.len(),
        warnings = response_errors.len(),
        "Fetched user media lists"
    );

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

async fn anilist_media_search(
    Query(params): Query<AnilistMediaSearchQuery>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let request_id = new_request_id();
    let (query, media_type) = match normalize_anilist_media_search_query(&params) {
        Ok(value) => value,
        Err(error) => {
            return (
                status_code_for_error(&error),
                [
                    ("content-type", "application/json"),
                    ("access-control-allow-origin", "*"),
                ],
                serde_json::json!({"error": public_error_message(&error)}).to_string(),
            )
                .into_response();
        }
    };

    let client = AnilistClient::with_client(state.http_client.clone());
    match client
        .search_media(&query, &media_type, ANILIST_MEDIA_SEARCH_MAX_RESULTS)
        .await
    {
        Ok(results) => {
            debug!(
                boot_id = %state.boot_id,
                request_id = %request_id,
                media_type = %media_type,
                result_count = results.len(),
                "Searched AniList media"
            );
            (
                StatusCode::OK,
                [
                    ("content-type", "application/json"),
                    ("access-control-allow-origin", "*"),
                ],
                serde_json::json!({"results": results}).to_string(),
            )
                .into_response()
        }
        Err(error) => {
            warn!(
                boot_id = %state.boot_id,
                request_id = %request_id,
                media_type = %media_type,
                error = %error,
                "AniList media search failed"
            );
            (
                status_code_for_error(&error),
                [
                    ("content-type", "application/json"),
                    ("access-control-allow-origin", "*"),
                ],
                serde_json::json!({"error": public_error_message(&error)}).to_string(),
            )
                .into_response()
        }
    }
}

async fn vndb_media_search(
    Query(params): Query<VndbMediaSearchQuery>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let request_id = new_request_id();
    let query = match normalize_vndb_media_search_query(&params) {
        Ok(value) => value,
        Err(error) => {
            return (
                status_code_for_error(&error),
                [
                    ("content-type", "application/json"),
                    ("access-control-allow-origin", "*"),
                ],
                serde_json::json!({"error": public_error_message(&error)}).to_string(),
            )
                .into_response();
        }
    };

    let client = VndbClient::with_client(state.http_client.clone());
    match client
        .search_vns(&query, VNDB_MEDIA_SEARCH_MAX_RESULTS)
        .await
    {
        Ok(results) => {
            debug!(
                boot_id = %state.boot_id,
                request_id = %request_id,
                result_count = results.len(),
                "Searched VNDB media"
            );
            (
                StatusCode::OK,
                [
                    ("content-type", "application/json"),
                    ("access-control-allow-origin", "*"),
                ],
                serde_json::json!({"results": results}).to_string(),
            )
                .into_response()
        }
        Err(error) => {
            warn!(
                boot_id = %state.boot_id,
                request_id = %request_id,
                error = %error,
                "VNDB media search failed"
            );
            (
                status_code_for_error(&error),
                [
                    ("content-type", "application/json"),
                    ("access-control-allow-origin", "*"),
                ],
                serde_json::json!({"error": public_error_message(&error)}).to_string(),
            )
                .into_response()
        }
    }
}

// === SSE progress stream endpoint ===

async fn generate_stream(
    Query(params): Query<GenerateStreamQuery>,
    State(state): State<AppState>,
) -> Sse<ReceiverStream<Result<Event, std::convert::Infallible>>> {
    let request_id = new_request_id();
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event, std::convert::Infallible>>(100);
    let settings = params.to_settings();
    let vndb_user = normalize_vndb_user_input(&params.vndb_user.unwrap_or_default());
    let anilist_user = normalize_anilist_user_input(&params.anilist_user.unwrap_or_default());
    let shelf_statuses = parse_shelf_status_params(
        params.vndb_status.as_deref(),
        params.anilist_status.as_deref(),
    );

    tokio::spawn(async move {
        let result = match shelf_statuses {
            Ok((vndb_statuses, anilist_statuses)) => {
                generate_dict_from_usernames(
                    &request_id,
                    &vndb_user,
                    &anilist_user,
                    &vndb_statuses,
                    &anilist_statuses,
                    settings,
                    Some(&tx),
                    &state,
                )
                .await
            }
            Err(error) => Err(error),
        };

        send_stream_result(&request_id, &state, &tx, result).await;
    });

    Sse::new(ReceiverStream::new(rx)).keep_alive(KeepAlive::default())
}

// === Download completed ZIP by token ===

async fn download_zip(
    Query(params): Query<DownloadQuery>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let request_id = new_request_id();
    let mut store = state.downloads.lock().await;

    if let Some(artifact) = store.remove(&params.token) {
        let zip_size_bytes = artifact.bytes.len();
        let disposition = format!("attachment; filename={}", artifact.filename);
        debug!(
            boot_id = %state.boot_id,
            request_id = %request_id,
            token = %params.token,
            zip_size_bytes = zip_size_bytes,
            filename = %artifact.filename,
            "Download token consumed"
        );
        (
            StatusCode::OK,
            download_headers(artifact.content_type, &disposition),
            artifact.bytes,
        )
            .into_response()
    } else {
        warn!(
            boot_id = %state.boot_id,
            request_id = %request_id,
            token = %params.token,
            "Download token not found"
        );
        (StatusCode::NOT_FOUND, "Download token not found or expired").into_response()
    }
}

fn download_headers(content_type: &'static str, disposition: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    if let Ok(value) = HeaderValue::from_str(disposition) {
        headers.insert(header::CONTENT_DISPOSITION, value);
    }
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_ORIGIN,
        HeaderValue::from_static("*"),
    );
    headers
}

async fn send_stream_result(
    request_id: &str,
    state: &AppState,
    tx: &tokio::sync::mpsc::Sender<Result<Event, std::convert::Infallible>>,
    result: Result<Vec<u8>, String>,
) {
    match result {
        Ok(zip_bytes) => {
            let artifact = DownloadArtifact::character_zip(zip_bytes);
            let zip_size_bytes = artifact.bytes.len();
            let token = store_download_artifact(state, artifact).await;
            info!(
                boot_id = %state.boot_id,
                request_id = %request_id,
                zip_size_bytes = zip_size_bytes,
                "SSE dictionary generation completed"
            );
            let _ = tx
                .send(Ok(Event::default()
                    .event("complete")
                    .data(serde_json::json!({"token": token}).to_string())))
                .await;
        }
        Err(error) => {
            warn!(
                boot_id = %state.boot_id,
                request_id = %request_id,
                error = %error,
                "SSE dictionary generation failed"
            );
            let _ = tx
                .send(Ok(Event::default().event("error").data(
                    serde_json::json!({
                        "error": public_error_message(&error),
                        "status": status_code_for_error(&error).as_u16()
                    })
                    .to_string(),
                )))
                .await;
        }
    }
}

async fn store_download_artifact(state: &AppState, artifact: DownloadArtifact) -> String {
    let token = uuid::Uuid::new_v4().to_string();
    let mut store = state.downloads.lock().await;
    let now = std::time::Instant::now();
    store.retain(|_, artifact| {
        now.duration_since(artifact.created_at).as_secs() < DOWNLOAD_TOKEN_MAX_AGE_SECS
    });
    store.insert(token.clone(), artifact);
    token
}

// === Jiten-backed frequency dictionary endpoints ===

async fn generate_frequency_dict(
    Query(params): Query<FrequencyQuery>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let request_id = new_request_id();
    let (download_url, index_url) = match frequency_urls(&params) {
        Ok(urls) => urls,
        Err(error) => return frequency_error_response(&error),
    };

    let entries = match media_entries_from_frequency_query(&state, &params).await {
        Ok((entries, _warnings)) => entries,
        Err(error) => return frequency_error_response(&error),
    };

    match generate_frequency_from_media_entries(
        &request_id,
        entries,
        &params,
        Some(download_url),
        Some(index_url),
        &state,
        None,
    )
    .await
    {
        Ok(result) => {
            info!(
                boot_id = %state.boot_id,
                request_id = %request_id,
                matched_count = result.matched_count,
                unmatched_count = result.unmatched.len(),
                term_count = result.total_terms,
                zip_size_bytes = result.zip_bytes.len(),
                "Frequency dictionary generation completed"
            );
            (
                StatusCode::OK,
                [
                    ("content-type", "application/zip"),
                    (
                        "content-disposition",
                        "attachment; filename=bee_frequency.zip",
                    ),
                    ("access-control-allow-origin", "*"),
                ],
                result.zip_bytes,
            )
                .into_response()
        }
        Err(error) => {
            warn!(
                boot_id = %state.boot_id,
                request_id = %request_id,
                error = %error,
                "Frequency dictionary generation failed"
            );
            frequency_error_response(&error)
        }
    }
}

async fn generate_frequency_index(Query(params): Query<FrequencyQuery>) -> impl IntoResponse {
    let (download_url, index_url) = match frequency_urls(&params) {
        Ok(urls) => urls,
        Err(error) => return frequency_error_response(&error),
    };
    let builder = FrequencyDictBuilder::new(Some(download_url), Some(index_url));
    (
        StatusCode::OK,
        [
            ("content-type", "application/json"),
            ("access-control-allow-origin", "*"),
        ],
        builder.create_index().to_string(),
    )
        .into_response()
}

async fn generate_frequency_stream(
    Query(params): Query<FrequencyQuery>,
    State(state): State<AppState>,
) -> Sse<ReceiverStream<Result<Event, std::convert::Infallible>>> {
    let request_id = new_request_id();
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event, std::convert::Infallible>>(100);

    tokio::spawn(async move {
        let result = async {
            let (download_url, index_url) = frequency_urls(&params)?;
            let (entries, warnings) = media_entries_from_frequency_query(&state, &params).await?;
            for warning in warnings {
                send_frequency_warning(&tx, serde_json::json!({ "message": warning })).await;
            }
            generate_frequency_from_media_entries(
                &request_id,
                entries,
                &params,
                Some(download_url),
                Some(index_url),
                &state,
                Some(&tx),
            )
            .await
        }
        .await;

        match result {
            Ok(result) => {
                let token = store_download_artifact(
                    &state,
                    DownloadArtifact::frequency_zip(result.zip_bytes),
                )
                .await;
                info!(
                    boot_id = %state.boot_id,
                    request_id = %request_id,
                    matched_count = result.matched_count,
                    unmatched_count = result.unmatched.len(),
                    term_count = result.total_terms,
                    "SSE frequency dictionary generation completed"
                );
                let _ = tx
                    .send(Ok(Event::default().event("complete").data(
                        serde_json::json!({
                            "token": token,
                            "filename": "bee_frequency.zip",
                            "matchedCount": result.matched_count,
                            "termCount": result.total_terms,
                            "unmatched": result.unmatched
                        })
                        .to_string(),
                    )))
                    .await;
            }
            Err(error) => {
                warn!(
                    boot_id = %state.boot_id,
                    request_id = %request_id,
                    error = %error,
                    "SSE frequency dictionary generation failed"
                );
                let _ = tx
                    .send(Ok(Event::default().event("error").data(
                        serde_json::json!({
                            "error": public_error_message(&error),
                            "status": status_code_for_error(&error).as_u16()
                        })
                        .to_string(),
                    )))
                    .await;
            }
        }
    });

    Sse::new(ReceiverStream::new(rx)).keep_alive(KeepAlive::default())
}

async fn generate_frequency_from_media_entries(
    _request_id: &str,
    entries: Vec<UserMediaEntry>,
    params: &FrequencyQuery,
    download_url: Option<String>,
    index_url: Option<String>,
    state: &AppState,
    progress_tx: Option<&tokio::sync::mpsc::Sender<Result<Event, std::convert::Infallible>>>,
) -> Result<FrequencyGenerationResult, String> {
    if entries.is_empty() {
        return Err(format!(
            "{} No selected VNDB/AniList media were provided for frequency generation",
            INVALID_INPUT_PREFIX
        ));
    }

    let mut unmatched = Vec::new();
    let mut deck_ids = std::collections::BTreeSet::new();
    let mut resolve_errors = Vec::new();
    let mut matched_count = 0usize;

    for (idx, entry) in entries.iter().enumerate() {
        send_frequency_progress(
            progress_tx,
            idx + 1,
            entries.len(),
            "Resolving Jiten decks",
            entry_display_title(entry),
        )
        .await;

        match state
            .jiten_client
            .deck_ids_by_external_link(&entry.source, &entry.id)
            .await
        {
            Ok(ids) if ids.is_empty() => unmatched.push(FrequencyUnmatchedMedia {
                media: entry.clone(),
                reason: "No matching Jiten frequency deck found".to_string(),
            }),
            Ok(ids) => {
                matched_count += 1;
                deck_ids.extend(ids);
            }
            Err(error) => {
                resolve_errors.push(error.clone());
                unmatched.push(FrequencyUnmatchedMedia {
                    media: entry.clone(),
                    reason: public_error_message(&error),
                });
            }
        }
    }

    if let Some(tx) = progress_tx {
        if !unmatched.is_empty() {
            send_frequency_warning(
                tx,
                serde_json::json!({
                    "message": "Some media did not have Jiten occurrence-count data.",
                    "unmatched": unmatched.clone()
                }),
            )
            .await;
        }
    }

    if deck_ids.is_empty() {
        return if resolve_errors.is_empty() {
            Err(format!(
                "{} No Jiten occurrence-count decks matched the selected titles",
                INVALID_INPUT_PREFIX
            ))
        } else {
            Err(combine_service_errors(&resolve_errors))
        };
    }

    let mut builder = FrequencyDictBuilder::new(download_url, index_url);
    let deck_ids: Vec<i32> = deck_ids.into_iter().collect();
    let mut deck_errors = Vec::new();
    let mut successful_decks = 0usize;

    for (idx, deck_id) in deck_ids.iter().enumerate() {
        send_frequency_progress(
            progress_tx,
            idx + 1,
            deck_ids.len(),
            "Downloading Jiten occurrence counts",
            &format!("Deck {}", deck_id),
        )
        .await;

        let zip_bytes = match state
            .jiten_client
            .download_yomitan_frequency_zip(*deck_id)
            .await
        {
            Ok(bytes) => bytes,
            Err(error) => {
                deck_errors.push(error);
                continue;
            }
        };

        let frequency_entries = match JitenClient::parse_yomitan_frequency_zip(&zip_bytes) {
            Ok(entries) => entries,
            Err(error) => {
                deck_errors.push(format!("UPSTREAM: {}", error));
                continue;
            }
        };

        builder.add_entries_for_deck(*deck_id, &frequency_entries);
        successful_decks += 1;
    }

    if !deck_errors.is_empty() {
        return Err(combine_service_errors(&deck_errors));
    }

    if successful_decks == 0 {
        return Err(combine_service_errors(&deck_errors));
    }

    if builder.is_empty() {
        return Err(format!(
            "{} Jiten decks were found, but no occurrence entries could be parsed",
            INVALID_INPUT_PREFIX
        ));
    }

    send_frequency_progress(
        progress_tx,
        1,
        1,
        "Building combined frequency dictionary",
        FREQUENCY_DICTIONARY_TITLE,
    )
    .await;

    let total_terms = builder.filtered_entry_count(params.min_occurrences, params.max_terms);
    if total_terms == 0 {
        return Err(format!(
            "{} No occurrence entries matched the requested filters",
            INVALID_INPUT_PREFIX
        ));
    }

    let zip_bytes = builder.export_bytes_with_options(
        params.min_occurrences,
        params.max_terms,
        params.display_mode,
        params.combine_mode,
    )?;
    Ok(FrequencyGenerationResult {
        zip_bytes,
        matched_count,
        unmatched,
        total_terms,
    })
}

async fn media_entries_from_frequency_query(
    state: &AppState,
    params: &FrequencyQuery,
) -> Result<(Vec<UserMediaEntry>, Vec<String>), String> {
    let mut entries = Vec::new();
    let mut warnings = Vec::new();
    let (vndb_statuses, anilist_statuses) = parse_shelf_status_params(
        params.vndb_status.as_deref(),
        params.anilist_status.as_deref(),
    )?;

    if let Some(entries_json) = params
        .entries
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        entries.extend(parse_frequency_entries_json(entries_json)?);
    }

    if params.vndb_user.as_deref().unwrap_or("").trim().is_empty()
        && params
            .anilist_user
            .as_deref()
            .unwrap_or("")
            .trim()
            .is_empty()
    {
        if entries.is_empty() {
            return Err(format!(
                "{} At least one username (vndb_user or anilist_user) or media entry is required",
                INVALID_INPUT_PREFIX
            ));
        }
    } else {
        let (user_entries, user_warnings) = collect_user_media_entries(
            state,
            params.vndb_user.as_deref(),
            params.anilist_user.as_deref(),
            &vndb_statuses,
            &anilist_statuses,
        )
        .await?;
        entries.extend(user_entries);
        warnings.extend(user_warnings);
    }

    let mut seen = HashSet::new();
    entries.retain(|entry| seen.insert((entry.source.clone(), entry.id.clone())));

    if entries.is_empty() {
        Err(format!(
            "{} No selected VNDB/AniList media were found for frequency generation",
            INVALID_INPUT_PREFIX
        ))
    } else {
        Ok((entries, warnings))
    }
}

fn parse_frequency_entries_json(entries_json: &str) -> Result<Vec<UserMediaEntry>, String> {
    let entries: Vec<ManualEntry> = serde_json::from_str(entries_json).map_err(|e| {
        format!(
            "{} entries must be a JSON array of source/id objects: {}",
            INVALID_INPUT_PREFIX, e
        )
    })?;

    let mut normalized = Vec::new();
    for entry in entries {
        let entry = normalize_manual_entry(&entry)?;
        let media_type = match (entry.source.as_str(), entry.media_type.as_str()) {
            ("vndb", _) => "vn",
            ("anilist", "MANGA") => "manga",
            ("anilist", _) => "anime",
            _ => "unknown",
        };
        normalized.push(UserMediaEntry {
            id: entry.id.clone(),
            title: entry.id.clone(),
            title_romaji: entry.id,
            source: entry.source,
            media_type: media_type.to_string(),
            status: entry.status,
        });
    }

    Ok(normalized)
}

fn frequency_urls(params: &FrequencyQuery) -> Result<(String, String), String> {
    if !has_frequency_media_input(params) {
        return Err(format!(
            "{} At least one username (vndb_user or anilist_user) or media entry is required",
            INVALID_INPUT_PREFIX
        ));
    }

    let parts = frequency_query_parts(params)?;
    let query = parts.join("&");
    let base = base_url();
    Ok((
        format!("{}/api/yomitan-frequency-dict?{}", base, query),
        format!("{}/api/yomitan-frequency-index?{}", base, query),
    ))
}

fn has_frequency_media_input(params: &FrequencyQuery) -> bool {
    params
        .vndb_user
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty())
        || params
            .anilist_user
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
        || params
            .entries
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
}

fn frequency_query_parts(params: &FrequencyQuery) -> Result<Vec<String>, String> {
    let mut parts = Vec::new();
    let (vndb_statuses, anilist_statuses) = parse_shelf_status_params(
        params.vndb_status.as_deref(),
        params.anilist_status.as_deref(),
    )?;

    let vndb_user = normalize_vndb_user_input(params.vndb_user.as_deref().unwrap_or(""));
    if !vndb_user.is_empty() {
        parts.push(format!("vndb_user={}", urlencoding::encode(&vndb_user)));
    }

    let anilist_user = normalize_anilist_user_input(params.anilist_user.as_deref().unwrap_or(""));
    if !anilist_user.is_empty() {
        parts.push(format!(
            "anilist_user={}",
            urlencoding::encode(&anilist_user)
        ));
    }
    append_shelf_status_params(
        &mut parts,
        !vndb_user.is_empty(),
        &vndb_statuses,
        !anilist_user.is_empty(),
        &anilist_statuses,
    );

    if let Some(entries) = params
        .entries
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        let normalized = normalize_entries_json_for_url(entries);
        parts.push(format!("entries={}", urlencoding::encode(&normalized)));
    }
    if let Some(min_occurrences) = params.min_occurrences {
        parts.push(format!("min_occurrences={}", min_occurrences));
    }
    if let Some(max_terms) = params.max_terms {
        parts.push(format!("max_terms={}", max_terms));
    }
    parts.push(format!(
        "display_mode={}",
        params.display_mode.as_query_value()
    ));
    parts.push(format!(
        "combine_mode={}",
        params.combine_mode.as_query_value()
    ));

    Ok(parts)
}

fn frequency_error_response(error: &str) -> axum::response::Response {
    (
        status_code_for_error(error),
        [
            ("content-type", "application/json"),
            ("access-control-allow-origin", "*"),
        ],
        serde_json::json!({"error": public_error_message(error)}).to_string(),
    )
        .into_response()
}

async fn send_frequency_progress(
    tx: Option<&tokio::sync::mpsc::Sender<Result<Event, std::convert::Infallible>>>,
    current: usize,
    total: usize,
    stage: &str,
    title: &str,
) {
    if let Some(tx) = tx {
        let _ = tx
            .send(Ok(Event::default().event("progress").data(
                serde_json::json!({
                    "current": current,
                    "total": total,
                    "stage": stage,
                    "title": title
                })
                .to_string(),
            )))
            .await;
    }
}

async fn send_frequency_warning(
    tx: &tokio::sync::mpsc::Sender<Result<Event, std::convert::Infallible>>,
    payload: serde_json::Value,
) {
    let _ = tx
        .send(Ok(Event::default()
            .event("warning")
            .data(payload.to_string())))
        .await;
}

fn entry_display_title(entry: &UserMediaEntry) -> &str {
    if !entry.title_romaji.trim().is_empty() {
        &entry.title_romaji
    } else if !entry.title.trim().is_empty() {
        &entry.title
    } else {
        &entry.id
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
    request_id: &str,
    vndb_user: &str,
    anilist_user: &str,
    vndb_statuses: &[VndbShelfStatus],
    anilist_statuses: &[AnilistShelfStatus],
    settings: DictSettings,
    progress_tx: Option<&tokio::sync::mpsc::Sender<Result<Event, std::convert::Infallible>>>,
    state: &AppState,
) -> Result<Vec<u8>, String> {
    let started_at = std::time::Instant::now();
    let vndb_user = normalize_vndb_user_input(vndb_user);
    let anilist_user = normalize_anilist_user_input(anilist_user);
    let mut stats = GenerationStats::default();
    let mut collected_errors: Vec<String> = Vec::new();
    let mut media_failures: Vec<MediaGenerationFailure> = Vec::new();

    // Step 1: Collect all media entries from user lists
    let mut media_entries: Vec<UserMediaEntry> = Vec::new();

    if !vndb_user.is_empty() {
        let client = VndbClient::with_client(state.http_client.clone());
        match client.fetch_user_list(&vndb_user, vndb_statuses).await {
            Ok(entries) => media_entries.extend(entries),
            Err(e) => {
                stats.record_failure(&e);
                collected_errors.push(e.clone());
                if anilist_user.is_empty() {
                    GenerationSummaryContext::new(
                        state,
                        request_id,
                        "usernames",
                        started_at,
                        &stats,
                        &media_failures,
                    )
                    .log(None, None, Some(&e));
                    return Err(e);
                }
                warn!(
                    boot_id = %state.boot_id,
                    request_id = %request_id,
                    user = %vndb_user,
                    error = %e,
                    "VNDB list fetch error (continuing)"
                );
            }
        }
    }

    if !anilist_user.is_empty() {
        let client = anilist_preview_client(state);
        match client
            .fetch_user_list(&anilist_user, anilist_statuses)
            .await
        {
            Ok(entries) => media_entries.extend(entries),
            Err(e) => {
                stats.record_failure(&e);
                collected_errors.push(e.clone());
                if vndb_user.is_empty() || media_entries.is_empty() {
                    GenerationSummaryContext::new(
                        state,
                        request_id,
                        "usernames",
                        started_at,
                        &stats,
                        &media_failures,
                    )
                    .log(None, None, Some(&e));
                    return Err(e);
                }
                warn!(
                    boot_id = %state.boot_id,
                    request_id = %request_id,
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
                "{} No selected media found in the requested user lists",
                INVALID_INPUT_PREFIX
            )
        } else {
            combine_service_errors(&collected_errors)
        };
        GenerationSummaryContext::new(
            state,
            request_id,
            "usernames",
            started_at,
            &stats,
            &media_failures,
        )
        .log(None, None, Some(&error));
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
    append_shelf_status_params(
        &mut url_parts,
        !vndb_user.is_empty(),
        vndb_statuses,
        !anilist_user.is_empty(),
        anilist_statuses,
    );
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
                        media_failures.push(MediaGenerationFailure::new(
                            &entry.source,
                            &entry.id,
                            &entry.title,
                            &e,
                        ));
                        warn!(
                            boot_id = %state.boot_id,
                            request_id = %request_id,
                            source = %entry.source,
                            media_id = %entry.id,
                            title = %entry.title,
                            error = %e,
                            "Failed to fetch VNDB characters"
                        );
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
                        media_failures.push(MediaGenerationFailure::new(
                            &entry.source,
                            &entry.id,
                            &entry.title,
                            &error,
                        ));
                        warn!(
                            boot_id = %state.boot_id,
                            request_id = %request_id,
                            source = %entry.source,
                            media_id = %entry.id,
                            title = %entry.title,
                            error = %error,
                            "Invalid AniList media ID"
                        );
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
                        media_failures.push(MediaGenerationFailure::new(
                            &entry.source,
                            &entry.id,
                            &entry.title,
                            &e,
                        ));
                        warn!(
                            boot_id = %state.boot_id,
                            request_id = %request_id,
                            source = %entry.source,
                            media_id = %entry.id,
                            title = %entry.title,
                            error = %e,
                            "Failed to fetch AniList characters"
                        );
                    }
                }
            }
            _ => {
                let error = "Internal error: unknown media source".to_string();
                collected_errors.push(error.clone());
                media_failures.push(MediaGenerationFailure::new(
                    &entry.source,
                    &entry.id,
                    &entry.title,
                    &error,
                ));
                warn!(
                    boot_id = %state.boot_id,
                    request_id = %request_id,
                    source = %entry.source,
                    media_id = %entry.id,
                    title = %entry.title,
                    error = %error,
                    "Unknown source"
                );
            }
        }
    }

    let log_context = GenerationSummaryContext::new(
        state,
        request_id,
        "usernames",
        started_at,
        &stats,
        &media_failures,
    );
    finalize_multi_media_generation(&mut builder, total, &collected_errors, &log_context)
}

// === Generate dictionary from multiple manual media entries ===

async fn generate_dict_from_entries(
    request_id: &str,
    entries: &[ManualEntry],
    settings: DictSettings,
    state: &AppState,
) -> Result<Vec<u8>, String> {
    let started_at = std::time::Instant::now();
    let mut stats = GenerationStats::default();
    let mut collected_errors: Vec<String> = Vec::new();
    let mut media_failures: Vec<MediaGenerationFailure> = Vec::new();

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
                media_failures.push(MediaGenerationFailure::new(
                    &entry.source,
                    &entry.id,
                    "",
                    &error,
                ));
                warn!(
                    boot_id = %state.boot_id,
                    request_id = %request_id,
                    source = %entry.source,
                    id = %entry.id,
                    error = %error,
                    "Skipping invalid manual entry"
                );
            }
        }
    }

    let total_requested = unique_entries.len() + media_failures.len();
    stats.media_total = total_requested;

    if unique_entries.is_empty() {
        let error = if !media_failures.is_empty() {
            build_multi_media_abort_error(total_requested, &media_failures, &collected_errors)
        } else {
            format!("{} No valid entries provided", INVALID_INPUT_PREFIX)
        };
        GenerationSummaryContext::new(
            state,
            request_id,
            "manual_entries",
            started_at,
            &stats,
            &media_failures,
        )
        .log(None, None, Some(&error));
        return Err(error);
    }

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
            if e.status != "current" {
                obj["status"] = serde_json::json!(e.status);
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

    let description = format!("Character Dictionary ({} titles)", total_requested);
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
                    media_failures.push(MediaGenerationFailure::new(
                        &entry.source,
                        &entry.id,
                        "",
                        &e,
                    ));
                    warn!(
                        boot_id = %state.boot_id,
                        request_id = %request_id,
                        source = %entry.source,
                        media_id = %entry.id,
                        error = %e,
                        "Failed to fetch VNDB characters"
                    );
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
                        media_failures.push(MediaGenerationFailure::new(
                            &entry.source,
                            &entry.id,
                            "",
                            &error,
                        ));
                        warn!(
                            boot_id = %state.boot_id,
                            request_id = %request_id,
                            source = %entry.source,
                            media_id = %entry.id,
                            error = %error,
                            "Invalid AniList media ID"
                        );
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
                        media_failures.push(MediaGenerationFailure::new(
                            &entry.source,
                            &entry.id,
                            "",
                            &e,
                        ));
                        warn!(
                            boot_id = %state.boot_id,
                            request_id = %request_id,
                            source = %entry.source,
                            media_id = %entry.id,
                            error = %e,
                            "Failed to fetch AniList characters"
                        );
                    }
                }
            }
            _ => {
                let error = "Internal error: unknown media source".to_string();
                collected_errors.push(error.clone());
                media_failures.push(MediaGenerationFailure::new(
                    &entry.source,
                    &entry.id,
                    "",
                    &error,
                ));
                warn!(
                    boot_id = %state.boot_id,
                    request_id = %request_id,
                    source = %entry.source,
                    media_id = %entry.id,
                    error = %error,
                    "Unknown source in entries"
                );
            }
        }
    }

    let log_context = GenerationSummaryContext::new(
        state,
        request_id,
        "manual_entries",
        started_at,
        &stats,
        &media_failures,
    );
    finalize_multi_media_generation(
        &mut builder,
        total_requested,
        &collected_errors,
        &log_context,
    )
}

// === Generate dictionary (single media OR username-based OR multi-entry) ===

async fn generate_dict(
    Query(params): Query<DictQuery>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let request_id = new_request_id();
    let settings = params.to_settings();

    let vndb_user = normalize_vndb_user_input(params.vndb_user.as_deref().unwrap_or(""));
    let anilist_user = normalize_anilist_user_input(params.anilist_user.as_deref().unwrap_or(""));
    let (vndb_statuses, anilist_statuses) = match parse_shelf_status_params(
        params.vndb_status.as_deref(),
        params.anilist_status.as_deref(),
    ) {
        Ok(statuses) => statuses,
        Err(error) => {
            return (status_code_for_error(&error), public_error_message(&error)).into_response();
        }
    };

    if !vndb_user.is_empty() || !anilist_user.is_empty() {
        match generate_dict_from_usernames(
            &request_id,
            &vndb_user,
            &anilist_user,
            &vndb_statuses,
            &anilist_statuses,
            settings,
            None,
            &state,
        )
        .await
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
                    request_id = %request_id,
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
        match generate_dict_from_entries(&request_id, &manual_entries, settings, &state).await {
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
                    request_id = %request_id,
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
            generate_vndb_dict(&request_id, &normalized_id, settings, &download_url, &state).await
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
            generate_anilist_dict(
                &request_id,
                media_id,
                &media_type,
                settings,
                &download_url,
                &state,
            )
            .await
        }
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                "source must be 'vndb' or 'anilist'",
            )
                .into_response();
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
                request_id = %request_id,
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
async fn generate_index(
    Query(params): Query<DictQuery>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let request_id = new_request_id();
    let vndb_user = normalize_vndb_user_input(params.vndb_user.as_deref().unwrap_or(""));
    let anilist_user = normalize_anilist_user_input(params.anilist_user.as_deref().unwrap_or(""));
    let (vndb_statuses, anilist_statuses) = match parse_shelf_status_params(
        params.vndb_status.as_deref(),
        params.anilist_status.as_deref(),
    ) {
        Ok(statuses) => statuses,
        Err(error) => {
            return (status_code_for_error(&error), public_error_message(&error)).into_response()
        }
    };

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
        append_shelf_status_params(
            &mut url_parts,
            !vndb_user.is_empty(),
            &vndb_statuses,
            !anilist_user.is_empty(),
            &anilist_statuses,
        );
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

    debug!(
        boot_id = %state.boot_id,
        request_id = %request_id,
        username_mode = !vndb_user.is_empty() || !anilist_user.is_empty(),
        entries_mode = params.entries.is_some(),
        "Generated Yomitan index metadata"
    );

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
    let vn_id =
        VndbClient::parse_vn_id(vn_id).map_err(|e| format!("{} {}", INVALID_INPUT_PREFIX, e))?;
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
        if let Some(va_info) = vn_info.va_map.get(&c.id) {
            c.seiyuu = Some(va_info.display_name.clone());
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
    let client = anilist_generation_client(state);
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
    request_id: &str,
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
            GenerationSummaryContext::new(
                state,
                request_id,
                "single_vndb",
                started_at,
                &stats,
                &[],
            )
            .log(None, None, Some(&error));
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
        GenerationSummaryContext::new(state, request_id, "single_vndb", started_at, &stats, &[])
            .log(Some(&builder), None, Some(&error));
        return Err(error);
    }

    let zip_bytes = match builder.export_bytes() {
        Ok(zip_bytes) => zip_bytes,
        Err(error) => {
            GenerationSummaryContext::new(
                state,
                request_id,
                "single_vndb",
                started_at,
                &stats,
                &[],
            )
            .log(Some(&builder), None, Some(&error));
            return Err(error);
        }
    };

    GenerationSummaryContext::new(state, request_id, "single_vndb", started_at, &stats, &[]).log(
        Some(&builder),
        Some(zip_bytes.len()),
        None,
    );

    Ok(zip_bytes)
}

async fn generate_anilist_dict(
    request_id: &str,
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
                GenerationSummaryContext::new(
                    state,
                    request_id,
                    "single_anilist",
                    started_at,
                    &stats,
                    &[],
                )
                .log(None, None, Some(&error));
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
        GenerationSummaryContext::new(state, request_id, "single_anilist", started_at, &stats, &[])
            .log(Some(&builder), None, Some(&error));
        return Err(error);
    }

    let zip_bytes = match builder.export_bytes() {
        Ok(zip_bytes) => zip_bytes,
        Err(error) => {
            GenerationSummaryContext::new(
                state,
                request_id,
                "single_anilist",
                started_at,
                &stats,
                &[],
            )
            .log(Some(&builder), None, Some(&error));
            return Err(error);
        }
    };

    GenerationSummaryContext::new(state, request_id, "single_anilist", started_at, &stats, &[])
        .log(Some(&builder), Some(zip_bytes.len()), None);

    Ok(zip_bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_test_state() -> (AppState, TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let image_cache = ImageCache::open(dir.path()).unwrap();
        let media_cache = MediaCache::open(dir.path()).unwrap();
        let http_client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(1))
            .build()
            .unwrap();
        let anilist_generation_http_client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(1))
            .build()
            .unwrap();

        (
            AppState {
                downloads: Arc::new(Mutex::new(HashMap::new())),
                jiten_client: JitenClient::new(http_client.clone()),
                http_client,
                anilist_generation_http_client,
                image_cache,
                media_cache,
                started_at: std::time::Instant::now(),
                boot_id: "test-boot-id".to_string(),
            },
            dir,
        )
    }

    fn make_cached_character_data() -> models::CharacterData {
        let mut data = models::CharacterData::new();
        data.main.push(models::Character {
            id: "c1".to_string(),
            name: "Okabe Rintarou".to_string(),
            name_original: "岡部 倫太郎".to_string(),
            role: "main".to_string(),
            source: "vndb".to_string(),
            ..models::Character::default()
        });
        data
    }

    #[test]
    fn test_status_code_for_invalid_input_error() {
        assert_eq!(
            status_code_for_error("INVALID_INPUT: bad request"),
            StatusCode::BAD_REQUEST
        );
    }

    #[test]
    fn test_normalize_anilist_media_search_query_accepts_trimmed_anime_query() {
        let query = AnilistMediaSearchQuery {
            q: Some("  steins gate  ".to_string()),
            media_type: Some("anime".to_string()),
        };

        let (search, media_type) = normalize_anilist_media_search_query(&query).unwrap();

        assert_eq!(search, "steins gate");
        assert_eq!(media_type, "ANIME");
    }

    #[test]
    fn test_normalize_anilist_media_search_query_accepts_manga() {
        let query = AnilistMediaSearchQuery {
            q: Some("berserk".to_string()),
            media_type: Some("MANGA".to_string()),
        };

        let (_, media_type) = normalize_anilist_media_search_query(&query).unwrap();

        assert_eq!(media_type, "MANGA");
    }

    #[test]
    fn test_normalize_anilist_media_search_query_rejects_short_query() {
        let query = AnilistMediaSearchQuery {
            q: Some("  a ".to_string()),
            media_type: Some("ANIME".to_string()),
        };

        let error = normalize_anilist_media_search_query(&query).unwrap_err();

        assert!(error.contains("at least 2"));
    }

    #[test]
    fn test_normalize_anilist_media_search_query_rejects_invalid_type() {
        let query = AnilistMediaSearchQuery {
            q: Some("naruto".to_string()),
            media_type: Some("NOVEL".to_string()),
        };

        let error = normalize_anilist_media_search_query(&query).unwrap_err();

        assert!(error.contains("ANIME or MANGA"));
    }

    #[test]
    fn test_normalize_vndb_media_search_query_accepts_trimmed_query() {
        let query = VndbMediaSearchQuery {
            q: Some("  ever17  ".to_string()),
        };

        let search = normalize_vndb_media_search_query(&query).unwrap();

        assert_eq!(search, "ever17");
    }

    #[test]
    fn test_normalize_vndb_media_search_query_rejects_short_query() {
        let query = VndbMediaSearchQuery {
            q: Some("  e ".to_string()),
        };

        let error = normalize_vndb_media_search_query(&query).unwrap_err();

        assert!(error.contains("at least 2"));
    }

    #[tokio::test]
    async fn test_vndb_media_search_rejects_short_query_response() {
        let (state, _dir) = make_test_state();

        let response = vndb_media_search(
            Query(VndbMediaSearchQuery {
                q: Some("  e ".to_string()),
            }),
            State(state),
        )
        .await
        .into_response();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body = String::from_utf8(body.to_vec()).unwrap();
        assert!(body.contains("at least 2"));
    }

    #[test]
    fn test_normalize_manual_entry_accepts_vndb_number() {
        let entry = ManualEntry {
            source: "vndb".to_string(),
            id: "17".to_string(),
            media_type: default_media_type(),
            status: default_manual_status(),
        };

        let normalized = normalize_manual_entry(&entry).unwrap();

        assert_eq!(normalized.source, "vndb");
        assert_eq!(normalized.id, "v17");
        assert_eq!(normalized.media_type, "vn");
    }

    #[test]
    fn test_normalize_manual_entry_accepts_vndb_url() {
        let entry = ManualEntry {
            source: "vndb".to_string(),
            id: "https://vndb.org/v17/steins-gate?view=chars".to_string(),
            media_type: default_media_type(),
            status: default_manual_status(),
        };

        let normalized = normalize_manual_entry(&entry).unwrap();

        assert_eq!(normalized.source, "vndb");
        assert_eq!(normalized.id, "v17");
        assert_eq!(normalized.media_type, "vn");
    }

    #[test]
    fn test_frequency_urls_require_media_input() {
        let error = frequency_urls(&FrequencyQuery::default()).unwrap_err();

        assert_eq!(status_code_for_error(&error), StatusCode::BAD_REQUEST);
        assert!(public_error_message(&error).contains("At least one username"));
        assert!(public_error_message(&error).contains("media entry"));
    }

    #[test]
    fn test_frequency_urls_preserve_query_params() {
        let params = FrequencyQuery {
            vndb_user: Some("https://vndb.org/u306797".to_string()),
            anilist_user: Some("Bee User".to_string()),
            vndb_status: Some("playing,finished,wishlist".to_string()),
            anilist_status: Some("current,completed,planning,paused,dropped".to_string()),
            entries: None,
            min_occurrences: Some(5),
            max_terms: Some(1000),
            ..FrequencyQuery::default()
        };

        let (download_url, index_url) = frequency_urls(&params).unwrap();

        assert!(download_url.contains("/api/yomitan-frequency-dict?"));
        assert!(index_url.contains("/api/yomitan-frequency-index?"));
        assert!(download_url.contains("vndb_user=u306797"));
        assert!(download_url.contains("anilist_user=Bee%20User"));
        assert!(download_url.contains("vndb_status=playing%2Cfinished%2Cwishlist"));
        assert!(download_url
            .contains("anilist_status=current%2Ccompleted%2Cplanning%2Cpaused%2Cdropped"));
        assert!(download_url.contains("min_occurrences=5"));
        assert!(download_url.contains("max_terms=1000"));
        assert!(download_url.contains("display_mode=occurrence"));
        assert!(download_url.contains("combine_mode=average"));
    }

    #[test]
    fn test_frequency_urls_accept_manual_entries_without_usernames() {
        let params = FrequencyQuery {
            vndb_user: None,
            anilist_user: None,
            entries: Some(
                r#"[{"source":"vndb","id":"17"},{"source":"anilist","id":"https://anilist.co/manga/30002","media_type":"MANGA"}]"#
                    .to_string(),
            ),
            min_occurrences: None,
            max_terms: None,
            ..FrequencyQuery::default()
        };

        let (download_url, index_url) = frequency_urls(&params).unwrap();

        assert!(download_url.contains("/api/yomitan-frequency-dict?"));
        assert!(index_url.contains("/api/yomitan-frequency-index?"));
        assert!(download_url.contains("entries="));
        assert!(download_url.contains("%22source%22%3A%22vndb%22"));
        assert!(download_url.contains("%22id%22%3A%22v17%22"));
        assert!(download_url.contains("%22source%22%3A%22anilist%22"));
        assert!(download_url.contains("%22id%22%3A%2230002%22"));
        assert!(download_url.contains("%22media_type%22%3A%22MANGA%22"));
        assert!(download_url.contains("combine_mode=average"));
    }

    #[test]
    fn test_frequency_urls_preserve_sum_combine_mode() {
        let params = FrequencyQuery {
            vndb_user: Some("Bee".to_string()),
            combine_mode: FrequencyCombineMode::Sum,
            display_mode: FrequencyDisplayMode::PerMillion,
            ..FrequencyQuery::default()
        };

        let (download_url, index_url) = frequency_urls(&params).unwrap();

        assert!(download_url.contains("display_mode=per_million"));
        assert!(download_url.contains("combine_mode=sum"));
        assert!(index_url.contains("display_mode=per_million"));
        assert!(index_url.contains("combine_mode=sum"));
    }

    #[test]
    fn test_frequency_urls_default_combine_mode_is_average() {
        let params = FrequencyQuery {
            vndb_user: Some("Bee".to_string()),
            ..FrequencyQuery::default()
        };

        let (download_url, index_url) = frequency_urls(&params).unwrap();

        assert!(download_url.contains("combine_mode=average"));
        assert!(index_url.contains("combine_mode=average"));
    }

    #[test]
    fn test_frequency_urls_omit_default_status_params() {
        let params = FrequencyQuery {
            vndb_user: Some("Bee".to_string()),
            anilist_user: Some("Bee".to_string()),
            ..FrequencyQuery::default()
        };

        let (download_url, index_url) = frequency_urls(&params).unwrap();

        assert!(!download_url.contains("vndb_status="));
        assert!(!download_url.contains("anilist_status="));
        assert!(!index_url.contains("vndb_status="));
        assert!(!index_url.contains("anilist_status="));
    }

    #[test]
    fn test_parse_shelf_status_params_defaults_to_current_only() {
        let (vndb_statuses, anilist_statuses) = parse_shelf_status_params(None, None).unwrap();

        assert_eq!(vndb_statuses, vec![VndbShelfStatus::Playing]);
        assert_eq!(anilist_statuses, vec![AnilistShelfStatus::Current]);
    }

    #[test]
    fn test_frequency_query_accepts_average_combine_mode() {
        let params: FrequencyQuery = serde_json::from_value(serde_json::json!({
            "vndb_user": "Bee",
            "display_mode": "rank",
            "combine_mode": "average"
        }))
        .unwrap();

        assert_eq!(params.display_mode, FrequencyDisplayMode::Rank);
        assert_eq!(params.combine_mode, FrequencyCombineMode::Average);
    }

    #[test]
    fn test_parse_frequency_entries_json_normalizes_manual_entries() {
        let entries = parse_frequency_entries_json(
            r#"[{"source":"vndb","id":"17"},{"source":"anilist","id":"https://anilist.co/anime/9253","media_type":"ANIME"}]"#,
        )
        .unwrap();

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].source, "vndb");
        assert_eq!(entries[0].id, "v17");
        assert_eq!(entries[0].media_type, "vn");
        assert_eq!(entries[0].status, "current");
        assert_eq!(entries[1].source, "anilist");
        assert_eq!(entries[1].id, "9253");
        assert_eq!(entries[1].media_type, "anime");
        assert_eq!(entries[1].status, "current");
    }

    #[test]
    fn test_frequency_index_metadata_title_and_update_urls() {
        let builder = FrequencyDictBuilder::new(
            Some("https://example.com/api/yomitan-frequency-dict?vndb_user=Bee".to_string()),
            Some("https://example.com/api/yomitan-frequency-index?vndb_user=Bee".to_string()),
        );

        let index = builder.create_index();

        assert_eq!(index["title"], FREQUENCY_DICTIONARY_TITLE);
        assert_eq!(index["format"], 3);
        assert_eq!(index["frequencyMode"], "occurrence-based");
        assert_eq!(
            index["attribution"],
            "Data from jiten.moe licensed under CC BY-SA 4.0."
        );
        assert_eq!(index["sourceUrl"], "https://jiten.moe/");
        assert_eq!(
            index["licenseUrl"],
            "https://creativecommons.org/licenses/by-sa/4.0/"
        );
        assert_eq!(index["isUpdatable"], true);
        assert!(index["downloadUrl"]
            .as_str()
            .unwrap()
            .contains("/api/yomitan-frequency-dict"));
        assert!(index["indexUrl"]
            .as_str()
            .unwrap()
            .contains("/api/yomitan-frequency-index"));
    }

    #[test]
    fn test_default_log_filter_suppresses_tower_governor_noise() {
        let filter = tracing_subscriber::EnvFilter::new(DEFAULT_LOG_FILTER);
        let rendered = filter.to_string();
        assert!(rendered.contains("info"));
        assert!(rendered.contains("tower_governor=warn"));
        assert!(DEFAULT_LOG_FILTER.contains("tower_governor=warn"));
    }

    #[test]
    fn test_anilist_client_helpers_select_expected_retry_profiles() {
        let (state, _dir) = make_test_state();

        assert_eq!(
            anilist_preview_client(&state).retry_policy(),
            AnilistRetryPolicy::Preview
        );
        assert_eq!(
            anilist_generation_client(&state).retry_policy(),
            AnilistRetryPolicy::Generation
        );
    }

    #[test]
    fn test_build_multi_media_abort_error_prefers_rate_limit() {
        let failures = vec![
            MediaGenerationFailure::new(
                "anilist",
                "9253",
                "Steins;Gate",
                "UPSTREAM: upstream down",
            ),
            MediaGenerationFailure::new("anilist", "30002", "Another", "RATE_LIMIT: slow down"),
        ];

        let error = build_multi_media_abort_error(2, &failures, &[]);
        assert_eq!(
            status_code_for_error(&error),
            StatusCode::SERVICE_UNAVAILABLE
        );
        assert_eq!(
            public_error_message(&error),
            "Dictionary generation aborted because 2 of 2 media failed."
        );
    }

    #[test]
    fn test_build_multi_media_abort_error_prefers_upstream_over_invalid_input() {
        let failures = vec![
            MediaGenerationFailure::new("anilist", "bad-id", "", "INVALID_INPUT: bad id"),
            MediaGenerationFailure::new("vndb", "v17", "Steins;Gate", "UPSTREAM: fetch failed"),
        ];

        let error = build_multi_media_abort_error(2, &failures, &[]);
        assert_eq!(status_code_for_error(&error), StatusCode::BAD_GATEWAY);
    }

    #[test]
    fn test_generation_summary_failure_context_caps_preview_and_keeps_request_id() {
        let failures: Vec<MediaGenerationFailure> = (0..6)
            .map(|idx| {
                MediaGenerationFailure::new(
                    "vndb",
                    &format!("v{}", idx),
                    &format!("Title {}", idx),
                    "UPSTREAM: failed",
                )
            })
            .collect();

        let (request_id, failed_media_count, preview) =
            generation_summary_failure_context("req-123", &failures);

        assert_eq!(request_id, "req-123");
        assert_eq!(failed_media_count, 6);
        assert_eq!(preview.len(), 5);
        assert_eq!(preview[0], "vndb:<redacted>");
        assert_eq!(preview[4], "vndb:<redacted>");
        assert!(!preview.iter().any(|item| item.contains("Title")));
        assert!(!preview.iter().any(|item| item.contains("v0")));
    }

    #[test]
    fn test_finalize_multi_media_generation_usernames_aborts_on_partial_failure() {
        let (state, _dir) = make_test_state();
        let mut builder = DictBuilder::new(
            DictSettings::default(),
            Some("https://example.com/api/yomitan-dict".to_string()),
            "Character Dictionary (2 titles)".to_string(),
        );
        let character = models::Character {
            id: "c1".to_string(),
            name: "Okabe Rintarou".to_string(),
            name_original: "岡部 倫太郎".to_string(),
            role: "main".to_string(),
            source: "vndb".to_string(),
            ..models::Character::default()
        };
        builder.add_character(&character, "Steins;Gate");

        let failures = vec![MediaGenerationFailure::new(
            "anilist",
            "9253",
            "Steins;Gate",
            "UPSTREAM: AniList request failed",
        )];
        let stats = GenerationStats {
            media_total: 2,
            ..GenerationStats::default()
        };

        let log_context = GenerationSummaryContext::new(
            &state,
            "req-usernames",
            "usernames",
            std::time::Instant::now(),
            &stats,
            &failures,
        );
        let error =
            finalize_multi_media_generation(&mut builder, 2, &[], &log_context).unwrap_err();

        assert_eq!(status_code_for_error(&error), StatusCode::BAD_GATEWAY);
        assert_eq!(
            public_error_message(&error),
            "Dictionary generation aborted because 1 of 2 media failed."
        );
    }

    #[tokio::test]
    async fn test_generate_dict_from_entries_partial_failure_returns_error() {
        let (state, _dir) = make_test_state();
        state
            .media_cache
            .put("vndb:v17", "Steins;Gate", &make_cached_character_data());

        let entries = vec![
            ManualEntry {
                source: "vndb".to_string(),
                id: "v17".to_string(),
                media_type: default_media_type(),
                status: default_manual_status(),
            },
            ManualEntry {
                source: "anilist".to_string(),
                id: "not-a-number".to_string(),
                media_type: "ANIME".to_string(),
                status: default_manual_status(),
            },
        ];

        let error =
            generate_dict_from_entries("req-entries", &entries, DictSettings::default(), &state)
                .await
                .unwrap_err();

        assert_eq!(status_code_for_error(&error), StatusCode::BAD_REQUEST);
        assert_eq!(
            public_error_message(&error),
            "Dictionary generation aborted because 1 of 2 media failed."
        );
    }

    #[tokio::test]
    async fn test_send_stream_result_error_emits_error_event_without_download_token() {
        let (state, _dir) = make_test_state();
        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        let error = format!(
            "{} Dictionary generation aborted because 1 of 2 media failed.",
            RATE_LIMIT_PREFIX
        );

        send_stream_result("req-stream", &state, &tx, Err(error.clone())).await;
        drop(tx);

        let event = rx.recv().await.unwrap().unwrap();
        let rendered = format!("{:?}", event);
        assert!(rendered.contains("event: error"));
        assert!(rendered.contains("Dictionary generation aborted because 1 of 2 media failed."));
        assert!(rx.recv().await.is_none());
        assert!(state.downloads.lock().await.is_empty());
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
                status: "playing".to_string(),
            },
            UserMediaEntry {
                id: "v17".to_string(),
                title: "Steins;Gate".to_string(),
                title_romaji: "Steins;Gate".to_string(),
                source: "vndb".to_string(),
                media_type: "vn".to_string(),
                status: "playing".to_string(),
            },
            UserMediaEntry {
                id: "9253".to_string(),
                title: "Steins;Gate".to_string(),
                title_romaji: "Steins;Gate".to_string(),
                source: "anilist".to_string(),
                media_type: "anime".to_string(),
                status: "current".to_string(),
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
                status: "playing".to_string(),
            },
            UserMediaEntry {
                id: "9253".to_string(),
                title: "Steins;Gate".to_string(),
                title_romaji: "Steins;Gate".to_string(),
                source: "anilist".to_string(),
                media_type: "anime".to_string(),
                status: "current".to_string(),
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
                status: "current".to_string(),
            },
            UserMediaEntry {
                id: "30002".to_string(),
                title: "Second Title".to_string(),
                title_romaji: "Second".to_string(),
                source: "anilist".to_string(),
                media_type: "manga".to_string(),
                status: "completed".to_string(),
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
            status: "playing".to_string(),
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
                status: "playing".to_string(),
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
                status: "playing".to_string(),
            },
            UserMediaEntry {
                id: "1".to_string(),
                title: "A".to_string(),
                title_romaji: "A".to_string(),
                source: "anilist".to_string(),
                media_type: "anime".to_string(),
                status: "current".to_string(),
            },
            UserMediaEntry {
                id: "2".to_string(),
                title: "B".to_string(),
                title_romaji: "B".to_string(),
                source: "vndb".to_string(),
                media_type: "vn".to_string(),
                status: "finished".to_string(),
            },
            UserMediaEntry {
                id: "2".to_string(),
                title: "B".to_string(),
                title_romaji: "B".to_string(),
                source: "anilist".to_string(),
                media_type: "manga".to_string(),
                status: "completed".to_string(),
            },
            UserMediaEntry {
                id: "1".to_string(),
                title: "A dup".to_string(),
                title_romaji: "A".to_string(),
                source: "vndb".to_string(),
                media_type: "vn".to_string(),
                status: "wishlist".to_string(),
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
            vndb_status: None,
            anilist_status: None,
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
            vndb_status: None,
            anilist_status: None,
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
