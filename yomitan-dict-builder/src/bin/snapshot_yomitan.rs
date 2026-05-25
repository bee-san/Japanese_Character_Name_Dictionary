use anyhow::Result;
use clap::{ArgAction, Parser};
use std::path::PathBuf;
use yomitan_dict_builder::{
    content_builder::DictSettings,
    snapshot_yomitan::{export_yomitan_from_snapshot, SnapshotYomitanOptions},
};

#[derive(Debug, Parser)]
#[command(name = "snapshot_yomitan")]
#[command(about = "Build a Yomitan dictionary ZIP from an offline snapshot.sqlite directory")]
struct Cli {
    #[arg(long)]
    snapshot: PathBuf,
    #[arg(long)]
    out: PathBuf,
    #[arg(long)]
    title: Option<String>,
    #[arg(long, default_value_t = true, action = ArgAction::Set)]
    show_image: bool,
    #[arg(long, default_value_t = true, action = ArgAction::Set)]
    show_tag: bool,
    #[arg(long, default_value_t = true, action = ArgAction::Set)]
    show_description: bool,
    #[arg(long, default_value_t = true, action = ArgAction::Set)]
    show_traits: bool,
    #[arg(long, default_value_t = true, action = ArgAction::Set)]
    show_spoilers: bool,
    #[arg(long, default_value_t = true, action = ArgAction::Set)]
    honorifics: bool,
    #[arg(long, default_value_t = true, action = ArgAction::Set)]
    show_seiyuu: bool,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let result = export_yomitan_from_snapshot(
        &cli.snapshot,
        &cli.out,
        SnapshotYomitanOptions {
            settings: DictSettings {
                show_image: cli.show_image,
                show_tag: cli.show_tag,
                show_description: cli.show_description,
                show_traits: cli.show_traits,
                show_spoilers: cli.show_spoilers,
                honorifics: cli.honorifics,
                show_seiyuu: cli.show_seiyuu,
            },
            title: cli.title,
        },
    )?;

    println!("zip: {}", result.zip_path.display());
    println!(
        "character_source_records: {}",
        result.character_source_records
    );
    println!(
        "character_entries_added: {}",
        result.character_entries_added
    );
    println!("generic_entries_added: {}", result.generic_entries_added);
    println!("skipped_no_japanese: {}", result.skipped_no_japanese);
    Ok(())
}
