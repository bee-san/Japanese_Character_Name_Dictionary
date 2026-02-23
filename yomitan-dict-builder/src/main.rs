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
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio_stream::wrappers::ReceiverStream;
use tower_http::services::ServeDir;

mod anilist_client;
mod content_builder;
mod dict_builder;
mod image_handler;
mod models;
mod name_parser;
mod vndb_client;

use anilist_client::AnilistClient;
use dict_builder::DictBuilder;
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

#[derive(Clone)]
struct AppState {
    downloads: DownloadStore,
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
}

#[derive(Deserialize)]
struct DownloadQuery {
    token: String,
}

fn default_media_type() -> String {
    "ANIME".to_string()
}

#[tokio::main]
async fn main() {
    let state = AppState {
        downloads: Arc::new(Mutex::new(HashMap::new())),
    };

    let app = Router::new()
        .route("/", get(serve_index))
        .route("/api/user-lists", get(fetch_user_lists))
        .route("/api/generate-stream", get(generate_stream))
        .route("/api/download", get(download_zip))
        .route("/api/yomitan-dict", get(generate_dict))
        .route("/api/yomitan-index", get(generate_index))
        .nest_service("/static", ServeDir::new(static_dir()))
        .with_state(state);

    let addr = "0.0.0.0:3000";
    println!("Server running on http://{}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
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

// === New endpoint: Fetch user lists ===

async fn fetch_user_lists(Query(params): Query<UserListQuery>) -> impl IntoResponse {
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

    // Fetch VNDB list
    if !vndb_user.is_empty() {
        let client = VndbClient::new();
        match client.fetch_user_playing_list(&vndb_user).await {
            Ok(entries) => all_entries.extend(entries),
            Err(e) => errors.push(format!("VNDB: {}", e)),
        }
    }

    // Fetch AniList list
    if !anilist_user.is_empty() {
        let client = AnilistClient::new();
        match client.fetch_user_current_list(&anilist_user).await {
            Ok(entries) => all_entries.extend(entries),
            Err(e) => errors.push(format!("AniList: {}", e)),
        }
    }

    // If both failed, return error
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

// === New endpoint: SSE progress stream for dictionary generation ===

async fn generate_stream(
    Query(params): Query<GenerateStreamQuery>,
    State(state): State<AppState>,
) -> Sse<ReceiverStream<Result<Event, std::convert::Infallible>>> {
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Event, std::convert::Infallible>>(100);
    let spoiler_level = params.spoiler_level.min(2);
    let vndb_user = params.vndb_user.unwrap_or_default().trim().to_string();
    let anilist_user = params.anilist_user.unwrap_or_default().trim().to_string();

    tokio::spawn(async move {
        let result = generate_dict_from_usernames(
            &vndb_user,
            &anilist_user,
            spoiler_level,
            Some(&tx),
        )
        .await;

        match result {
            Ok(zip_bytes) => {
                // Store ZIP in temp storage
                let token = uuid::Uuid::new_v4().to_string();

                // Clean up old entries (older than 5 minutes)
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

// === New endpoint: Download completed ZIP by token ===

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
                    "attachment; filename=gsm_characters.zip",
                ),
                ("access-control-allow-origin", "*"),
            ],
            zip_bytes,
        )
            .into_response()
    } else {
        (
            StatusCode::NOT_FOUND,
            "Download token not found or expired",
        )
            .into_response()
    }
}

// === Core function: Generate dictionary from usernames ===

async fn generate_dict_from_usernames(
    vndb_user: &str,
    anilist_user: &str,
    spoiler_level: u8,
    progress_tx: Option<&tokio::sync::mpsc::Sender<Result<Event, std::convert::Infallible>>>,
) -> Result<Vec<u8>, String> {
    // Step 1: Collect all media entries from user lists
    let mut media_entries: Vec<UserMediaEntry> = Vec::new();

    if !vndb_user.is_empty() {
        let client = VndbClient::new();
        match client.fetch_user_playing_list(vndb_user).await {
            Ok(entries) => media_entries.extend(entries),
            Err(e) => {
                if anilist_user.is_empty() {
                    return Err(format!("VNDB error: {}", e));
                }
                // Log but continue if we have AniList too
                eprintln!("VNDB list fetch error (continuing): {}", e);
            }
        }
    }

    if !anilist_user.is_empty() {
        let client = AnilistClient::new();
        match client.fetch_user_current_list(anilist_user).await {
            Ok(entries) => media_entries.extend(entries),
            Err(e) => {
                if vndb_user.is_empty() || media_entries.is_empty() {
                    return Err(format!("AniList error: {}", e));
                }
                eprintln!("AniList list fetch error (continuing): {}", e);
            }
        }
    }

    if media_entries.is_empty() {
        return Err("No in-progress media found in user lists".to_string());
    }

    let total = media_entries.len();

    // Build download URL with usernames for auto-update
    let mut url_parts = Vec::new();
    if !vndb_user.is_empty() {
        url_parts.push(format!("vndb_user={}", vndb_user));
    }
    if !anilist_user.is_empty() {
        url_parts.push(format!("anilist_user={}", anilist_user));
    }
    url_parts.push(format!("spoiler_level={}", spoiler_level));
    let download_url = format!(
        "http://127.0.0.1:3000/api/yomitan-dict?{}",
        url_parts.join("&")
    );

    // Build description
    let description = format!("Character Dictionary ({} titles)", total);

    let mut builder = DictBuilder::new(
        spoiler_level,
        Some(download_url),
        description,
    );

    // Step 2: For each media, fetch characters and add to dictionary
    for (i, entry) in media_entries.iter().enumerate() {
        let display_title = if !entry.title_romaji.is_empty() {
            &entry.title_romaji
        } else {
            &entry.title
        };

        // Send progress
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
                let client = VndbClient::new();

                // Fetch title (try to get Japanese title)
                let title = match client.fetch_vn_title(&entry.id).await {
                    Ok((romaji, original)) => {
                        if !original.is_empty() {
                            original
                        } else {
                            romaji
                        }
                    }
                    Err(_) => game_title.clone(),
                };

                // Fetch characters
                match client.fetch_characters(&entry.id).await {
                    Ok(mut char_data) => {
                        // Download images
                        for character in char_data.all_characters_mut() {
                            if let Some(ref url) = character.image_url {
                                character.image_base64 =
                                    client.fetch_image_as_base64(url).await;
                            }
                        }

                        // Add all characters to dictionary
                        for character in char_data.all_characters() {
                            builder.add_character(character, &title);
                        }
                    }
                    Err(e) => {
                        eprintln!(
                            "Failed to fetch characters for VNDB {}: {}",
                            entry.id, e
                        );
                    }
                }

                // Rate limit
                tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
            }
            "anilist" => {
                let client = AnilistClient::new();
                let media_id: i32 = match entry.id.parse() {
                    Ok(id) => id,
                    Err(_) => {
                        eprintln!("Invalid AniList media ID: {}", entry.id);
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

                        // Download images
                        for character in char_data.all_characters_mut() {
                            if let Some(ref url) = character.image_url {
                                character.image_base64 =
                                    client.fetch_image_as_base64(url).await;
                            }
                        }

                        // Add all characters
                        for character in char_data.all_characters() {
                            builder.add_character(character, &title);
                        }
                    }
                    Err(e) => {
                        eprintln!(
                            "Failed to fetch characters for AniList {}: {}",
                            entry.id, e
                        );
                    }
                }

                // Rate limit
                tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;
            }
            _ => {
                eprintln!("Unknown source: {}", entry.source);
            }
        }
    }

    if builder.entries.is_empty() {
        return Err("No character entries generated from any media".to_string());
    }

    Ok(builder.export_bytes())
}

// === Existing endpoint: Generate dictionary (single media OR username-based) ===

async fn generate_dict(Query(params): Query<DictQuery>) -> impl IntoResponse {
    let spoiler_level = params.spoiler_level.min(2);

    // Check if this is a username-based request
    let vndb_user = params.vndb_user.as_deref().unwrap_or("").trim().to_string();
    let anilist_user = params.anilist_user.as_deref().unwrap_or("").trim().to_string();

    if !vndb_user.is_empty() || !anilist_user.is_empty() {
        // Username-based generation (for Yomitan auto-update)
        match generate_dict_from_usernames(&vndb_user, &anilist_user, spoiler_level, None).await {
            Ok(bytes) => {
                return (
                    StatusCode::OK,
                    [
                        ("content-type", "application/zip"),
                        (
                            "content-disposition",
                            "attachment; filename=gsm_characters.zip",
                        ),
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

    // Single-media mode (existing behavior)
    let source = params.source.as_deref().unwrap_or("");
    let id = params.id.as_deref().unwrap_or("");

    if source.is_empty() || id.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            "Either provide source+id or vndb_user/anilist_user",
        )
            .into_response();
    }

    let download_url = format!(
        "http://127.0.0.1:3000/api/yomitan-dict?source={}&id={}&spoiler_level={}&media_type={}",
        source, id, spoiler_level, params.media_type
    );

    match source.to_lowercase().as_str() {
        "vndb" => match generate_vndb_dict(id, spoiler_level, &download_url).await {
            Ok(bytes) => (
                StatusCode::OK,
                [
                    ("content-type", "application/zip"),
                    (
                        "content-disposition",
                        "attachment; filename=gsm_characters.zip",
                    ),
                    ("access-control-allow-origin", "*"),
                ],
                bytes,
            )
                .into_response(),
            Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
        },
        "anilist" => {
            let media_id: i32 = match id.parse() {
                Ok(id) => id,
                Err(_) => {
                    return (
                        StatusCode::BAD_REQUEST,
                        "Invalid AniList ID: must be a number",
                    )
                        .into_response()
                }
            };
            let media_type = params.media_type.to_uppercase();
            if media_type != "ANIME" && media_type != "MANGA" {
                return (
                    StatusCode::BAD_REQUEST,
                    "media_type must be ANIME or MANGA",
                )
                    .into_response();
            }
            match generate_anilist_dict(media_id, &media_type, spoiler_level, &download_url).await
            {
                Ok(bytes) => (
                    StatusCode::OK,
                    [
                        ("content-type", "application/zip"),
                        (
                            "content-disposition",
                            "attachment; filename=gsm_characters.zip",
                        ),
                        ("access-control-allow-origin", "*"),
                    ],
                    bytes,
                )
                    .into_response(),
                Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e).into_response(),
            }
        }
        _ => (
            StatusCode::BAD_REQUEST,
            "source must be 'vndb' or 'anilist'",
        )
            .into_response(),
    }
}

/// Lightweight endpoint: returns just the index.json metadata as JSON.
async fn generate_index(Query(params): Query<DictQuery>) -> impl IntoResponse {
    let spoiler_level = params.spoiler_level.min(2);

    let vndb_user = params.vndb_user.as_deref().unwrap_or("").trim().to_string();
    let anilist_user = params.anilist_user.as_deref().unwrap_or("").trim().to_string();

    let download_url = if !vndb_user.is_empty() || !anilist_user.is_empty() {
        let mut url_parts = Vec::new();
        if !vndb_user.is_empty() {
            url_parts.push(format!("vndb_user={}", vndb_user));
        }
        if !anilist_user.is_empty() {
            url_parts.push(format!("anilist_user={}", anilist_user));
        }
        url_parts.push(format!("spoiler_level={}", spoiler_level));
        format!(
            "http://127.0.0.1:3000/api/yomitan-dict?{}",
            url_parts.join("&")
        )
    } else {
        let source = params.source.as_deref().unwrap_or("");
        let id = params.id.as_deref().unwrap_or("");
        format!(
            "http://127.0.0.1:3000/api/yomitan-dict?source={}&id={}&spoiler_level={}&media_type={}",
            source, id, spoiler_level, params.media_type
        )
    };

    let builder = DictBuilder::new(spoiler_level, Some(download_url), String::new());
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

// === Existing single-media helpers ===

async fn generate_vndb_dict(
    vn_id: &str,
    spoiler_level: u8,
    download_url: &str,
) -> Result<Vec<u8>, String> {
    let client = VndbClient::new();

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

    for character in char_data.all_characters_mut() {
        if let Some(ref url) = character.image_url {
            character.image_base64 = client.fetch_image_as_base64(url).await;
        }
    }

    let mut builder = DictBuilder::new(
        spoiler_level,
        Some(download_url.to_string()),
        game_title.clone(),
    );

    for character in char_data.all_characters() {
        builder.add_character(character, &game_title);
    }

    if builder.entries.is_empty() {
        return Err("No character entries generated".to_string());
    }

    Ok(builder.export_bytes())
}

async fn generate_anilist_dict(
    media_id: i32,
    media_type: &str,
    spoiler_level: u8,
    download_url: &str,
) -> Result<Vec<u8>, String> {
    let client = AnilistClient::new();

    let (mut char_data, media_title) = client.fetch_characters(media_id, media_type).await?;

    let game_title = if !media_title.is_empty() {
        media_title
    } else {
        format!("AniList {}", media_id)
    };

    for character in char_data.all_characters_mut() {
        if let Some(ref url) = character.image_url {
            character.image_base64 = client.fetch_image_as_base64(url).await;
        }
    }

    let mut builder = DictBuilder::new(
        spoiler_level,
        Some(download_url.to_string()),
        game_title.clone(),
    );

    for character in char_data.all_characters() {
        builder.add_character(character, &game_title);
    }

    if builder.entries.is_empty() {
        return Err("No character entries generated".to_string());
    }

    Ok(builder.export_bytes())
}