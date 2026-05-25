use anyhow::{Context, Result};
use chrono::Utc;
use clap::Parser;
use std::{fs, path::PathBuf, time::Duration};
use yomitan_dict_builder::{snapshot::vndb::build_vndb_raw_bundle, vndb_client::VndbClient};

#[derive(Parser)]
#[command(name = "vndb_snapshot_seed")]
#[command(about = "Fetch a VN from VNDB and write an offline raw snapshot bundle")]
struct Cli {
    #[arg(long)]
    vn: String,
    #[arg(long)]
    out: PathBuf,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .context("failed to build HTTP client")?;
    let vndb = VndbClient::with_client(client);

    let vn_id = VndbClient::parse_vn_id(&cli.vn)
        .map_err(anyhow::Error::msg)
        .with_context(|| format!("invalid VNDB VN input {}", cli.vn))?;
    let vn_info = vndb
        .fetch_vn_info(&vn_id)
        .await
        .map_err(anyhow::Error::msg)
        .with_context(|| format!("failed to fetch VN metadata for {vn_id}"))?;
    let mut char_data = vndb
        .fetch_characters(&vn_id)
        .await
        .map_err(anyhow::Error::msg)
        .with_context(|| format!("failed to fetch VN characters for {vn_id}"))?;

    for character in char_data.all_characters_mut() {
        if let Some(va_info) = vn_info.va_map.get(&character.id) {
            character.seiyuu = Some(va_info.display_name.clone());
        }
    }

    let retrieved_at = Utc::now().format("%Y-%m-%d").to_string();
    let bundle = build_vndb_raw_bundle(&vn_id, &vn_info, &char_data, &retrieved_at);

    if let Some(parent) = cli.out.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    fs::write(&cli.out, serde_json::to_string_pretty(&bundle)?)
        .with_context(|| format!("failed to write {}", cli.out.display()))?;

    println!("wrote {}", cli.out.display());
    println!("vn: {vn_id}");
    println!("records: {}", bundle.records.len());
    println!("characters: {}", char_data.all_characters().count());
    println!("voice_actors: {}", vn_info.va_map.len());
    Ok(())
}
