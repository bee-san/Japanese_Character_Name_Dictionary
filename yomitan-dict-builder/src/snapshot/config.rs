use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fs, path::Path};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotConfig {
    #[serde(default)]
    pub build: BuildConfig,
    #[serde(default)]
    pub image_policy: ImagePolicyConfig,
    #[serde(default)]
    pub sources: BTreeMap<String, SourceConfig>,
}

impl SnapshotConfig {
    pub fn load(path: &Path) -> Result<Self> {
        let raw = fs::read_to_string(path)
            .with_context(|| format!("failed to read config {}", path.display()))?;
        let config: SnapshotConfig = toml::from_str(&raw)
            .with_context(|| format!("failed to parse TOML {}", path.display()))?;
        Ok(config)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildConfig {
    #[serde(default)]
    pub snapshot_version: Option<String>,
    #[serde(default = "default_true")]
    pub fail_on_policy_error: bool,
    #[serde(default = "default_true")]
    pub fail_on_no_enabled_sources: bool,
    #[serde(default = "default_true")]
    pub fail_on_missing_input: bool,
}

impl Default for BuildConfig {
    fn default() -> Self {
        Self {
            snapshot_version: None,
            fail_on_policy_error: true,
            fail_on_no_enabled_sources: true,
            fail_on_missing_input: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImagePolicyConfig {
    #[serde(default = "default_true")]
    pub mirror_locally: bool,
    #[serde(default)]
    pub fetch_remote: bool,
    #[serde(default = "default_image_subdir")]
    pub image_store_subdir: String,
}

impl Default for ImagePolicyConfig {
    fn default() -> Self {
        Self {
            mirror_locally: true,
            fetch_remote: false,
            image_store_subdir: default_image_subdir(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceConfig {
    pub kind: String,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub order: Option<u32>,
    #[serde(default)]
    pub stage: Option<SourceStage>,
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub input: Option<String>,
    #[serde(default)]
    pub format: Option<String>,
    #[serde(default)]
    pub license_class: Option<String>,
    #[serde(default)]
    pub license_label: Option<String>,
    #[serde(default)]
    pub license_url: Option<String>,
    #[serde(default)]
    pub notes: Option<String>,
    #[serde(default)]
    pub rate_limit_per_minute: Option<u32>,
}

impl SourceConfig {
    pub fn resolved_stage(&self) -> SourceStage {
        self.stage.unwrap_or_else(|| {
            if self.enabled {
                SourceStage::EnabledNow
            } else {
                SourceStage::Later
            }
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceStage {
    EnabledNow,
    Later,
    DoNotTouchYet,
}

impl SourceStage {
    pub fn as_str(self) -> &'static str {
        match self {
            SourceStage::EnabledNow => "enabled_now",
            SourceStage::Later => "later",
            SourceStage::DoNotTouchYet => "do_not_touch_yet",
        }
    }
}

fn default_true() -> bool {
    true
}

fn default_image_subdir() -> String {
    "image_store".to_string()
}
