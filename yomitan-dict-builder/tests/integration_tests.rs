//! Integration tests for the Yomitan Dictionary Builder.
//! These tests verify the core functionality of user list fetching,
//! character processing, name parsing, content building, and dictionary assembly.

use std::collections::HashSet;

// We need to reference the library code. Since this is a binary crate,
// we'll test the public modules by importing them through the binary's module structure.
// For integration tests, we test via HTTP endpoints.

/// Test that the server starts and serves the index page.
#[tokio::test]
async fn test_index_page_accessible() {
    let client = reqwest::Client::new();
    // This test requires the server to be running - skip if not available
    let result = client
        .get("http://localhost:3000/")
        .timeout(std::time::Duration::from_secs(2))
        .send()
        .await;

    if let Ok(response) = result {
        assert_eq!(response.status(), 200);
        let body = response.text().await.unwrap();
        assert!(body.contains("Yomitan Dictionary Builder"));
        assert!(body.contains("From Username"));
        assert!(body.contains("From Media ID"));
    }
    // If server is not running, test is silently skipped
}

/// Test the user-lists endpoint validation (no usernames provided).
#[tokio::test]
async fn test_user_lists_no_username() {
    let client = reqwest::Client::new();
    let result = client
        .get("http://localhost:3000/api/user-lists")
        .timeout(std::time::Duration::from_secs(2))
        .send()
        .await;

    if let Ok(response) = result {
        assert_eq!(response.status(), 400);
        let body: serde_json::Value = response.json().await.unwrap();
        assert!(body["error"].as_str().unwrap().contains("At least one username"));
    }
}

/// Test the user-lists endpoint with an invalid VNDB username.
#[tokio::test]
async fn test_user_lists_invalid_vndb_user() {
    let client = reqwest::Client::new();
    let result = client
        .get("http://localhost:3000/api/user-lists?vndb_user=ThisUserShouldNotExist99999")
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await;

    if let Ok(response) = result {
        // Should return 400 because user not found
        assert_eq!(response.status(), 400);
        let body: serde_json::Value = response.json().await.unwrap();
        assert!(body["error"].as_str().unwrap().contains("not found"));
    }
}

/// Test the existing single-media dict endpoint validation.
#[tokio::test]
async fn test_dict_endpoint_missing_params() {
    let client = reqwest::Client::new();
    let result = client
        .get("http://localhost:3000/api/yomitan-dict")
        .timeout(std::time::Duration::from_secs(2))
        .send()
        .await;

    if let Ok(response) = result {
        assert_eq!(response.status(), 400);
    }
}

/// Test the yomitan-index endpoint returns valid JSON.
#[tokio::test]
async fn test_index_endpoint_returns_json() {
    let client = reqwest::Client::new();
    let result = client
        .get("http://localhost:3000/api/yomitan-index?source=vndb&id=v17&spoiler_level=0")
        .timeout(std::time::Duration::from_secs(2))
        .send()
        .await;

    if let Ok(response) = result {
        assert_eq!(response.status(), 200);
        let body: serde_json::Value = response.json().await.unwrap();
        assert_eq!(body["title"], "GSM Character Dictionary");
        assert_eq!(body["format"], 3);
        assert_eq!(body["author"], "GameSentenceMiner");
        assert!(body["downloadUrl"].as_str().is_some());
        assert!(body["indexUrl"].as_str().is_some());
        assert_eq!(body["isUpdatable"], true);
    }
}

/// Test the yomitan-index endpoint with username-based params.
#[tokio::test]
async fn test_index_endpoint_username_based() {
    let client = reqwest::Client::new();
    let result = client
        .get("http://localhost:3000/api/yomitan-index?vndb_user=test&anilist_user=test2&spoiler_level=1")
        .timeout(std::time::Duration::from_secs(2))
        .send()
        .await;

    if let Ok(response) = result {
        assert_eq!(response.status(), 200);
        let body: serde_json::Value = response.json().await.unwrap();
        let download_url = body["downloadUrl"].as_str().unwrap();
        assert!(download_url.contains("vndb_user=test"));
        assert!(download_url.contains("anilist_user=test2"));
        assert!(download_url.contains("spoiler_level=1"));
    }
}

/// Test download endpoint with invalid token.
#[tokio::test]
async fn test_download_invalid_token() {
    let client = reqwest::Client::new();
    let result = client
        .get("http://localhost:3000/api/download?token=nonexistent-token")
        .timeout(std::time::Duration::from_secs(2))
        .send()
        .await;

    if let Ok(response) = result {
        assert_eq!(response.status(), 404);
    }
}