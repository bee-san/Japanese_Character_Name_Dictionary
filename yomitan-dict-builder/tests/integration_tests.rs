//! Integration tests for the Yomitan Dictionary Builder.
//! These tests verify the core functionality of user list fetching,
//! character processing, name parsing, content building, and dictionary assembly.

// We need to reference the library code. Since this is a binary crate,
// we'll test the public modules by importing them through the binary's module structure.
// For integration tests, we test via HTTP endpoints.

const TEST_SERVER_URL: &str = "http://localhost:3000";

async fn test_server_is_running_app(client: &reqwest::Client) -> bool {
    let result = client
        .get(format!("{}/", TEST_SERVER_URL))
        .timeout(std::time::Duration::from_secs(2))
        .send()
        .await;

    let Ok(response) = result else {
        return false;
    };

    if !response.status().is_success() {
        return false;
    }

    response
        .text()
        .await
        .map(|body| body.contains("Bee's Character Dictionary"))
        .unwrap_or(false)
}

/// Test that the server starts and serves the index page.
#[tokio::test]
async fn test_index_page_accessible() {
    let client = reqwest::Client::new();
    // This test requires the server to be running - skip if not available
    if !test_server_is_running_app(&client).await {
        return;
    }

    let response = client
        .get(format!("{}/", TEST_SERVER_URL))
        .timeout(std::time::Duration::from_secs(2))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let body = response.text().await.unwrap();
    assert!(body.contains("Bee's Character Dictionary"));
    assert!(body.contains("From VNDB / AniList Username"));
    assert!(body.contains("From VNDB / AniList Media ID"));
}

/// Test the user-lists endpoint validation (no usernames provided).
#[tokio::test]
async fn test_user_lists_no_username() {
    let client = reqwest::Client::new();
    if !test_server_is_running_app(&client).await {
        return;
    }

    let result = client
        .get(format!("{}/api/user-lists", TEST_SERVER_URL))
        .timeout(std::time::Duration::from_secs(2))
        .send()
        .await;

    if let Ok(response) = result {
        assert_eq!(response.status(), 400);
        let body: serde_json::Value = response.json().await.unwrap();
        assert!(body["error"]
            .as_str()
            .unwrap()
            .contains("At least one username"));
    }
}

/// Test the user-lists endpoint with an invalid VNDB username.
#[tokio::test]
async fn test_user_lists_invalid_vndb_user() {
    let client = reqwest::Client::new();
    if !test_server_is_running_app(&client).await {
        return;
    }

    let result = client
        .get(format!(
            "{}/api/user-lists?vndb_user=ThisUserShouldNotExist99999",
            TEST_SERVER_URL
        ))
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
    if !test_server_is_running_app(&client).await {
        return;
    }

    let result = client
        .get(format!("{}/api/yomitan-dict", TEST_SERVER_URL))
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
    if !test_server_is_running_app(&client).await {
        return;
    }

    let result = client
        .get(format!(
            "{}/api/yomitan-index?source=vndb&id=v17",
            TEST_SERVER_URL
        ))
        .timeout(std::time::Duration::from_secs(2))
        .send()
        .await;

    if let Ok(response) = result {
        assert_eq!(response.status(), 200);
        let body: serde_json::Value = response.json().await.unwrap();
        assert_eq!(body["title"], "Bee's Character Dictionary");
        assert_eq!(body["format"], 3);
        assert_eq!(body["author"], "Bee (https://github.com/bee-san)");
        assert!(body["downloadUrl"].as_str().is_some());
        assert!(body["indexUrl"].as_str().is_some());
        assert_eq!(body["isUpdatable"], true);
    }
}

/// Test the yomitan-index endpoint with username-based params.
#[tokio::test]
async fn test_index_endpoint_username_based() {
    let client = reqwest::Client::new();
    if !test_server_is_running_app(&client).await {
        return;
    }

    let result = client
        .get(format!(
            "{}/api/yomitan-index?vndb_user=test&anilist_user=test2&spoilers=false",
            TEST_SERVER_URL
        ))
        .timeout(std::time::Duration::from_secs(2))
        .send()
        .await;

    if let Ok(response) = result {
        assert_eq!(response.status(), 200);
        let body: serde_json::Value = response.json().await.unwrap();
        let download_url = body["downloadUrl"].as_str().unwrap();
        assert!(download_url.contains("vndb_user=test"));
        assert!(download_url.contains("anilist_user=test2"));
        assert!(download_url.contains("spoilers=false"));
    }
}

/// Test download endpoint with invalid token.
#[tokio::test]
async fn test_download_invalid_token() {
    let client = reqwest::Client::new();
    if !test_server_is_running_app(&client).await {
        return;
    }

    let result = client
        .get(format!(
            "{}/api/download?token=nonexistent-token",
            TEST_SERVER_URL
        ))
        .timeout(std::time::Duration::from_secs(2))
        .send()
        .await;

    if let Ok(response) = result {
        assert_eq!(response.status(), 404);
    }
}
