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

/// Shared application state for temporary ZIP storage.
