use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use yomitan_dict_builder::snapshot::pipeline::{
    build_snapshot, regenerate_reports, verify_output_dir,
};

#[derive(Parser)]
#[command(name = "ultimate_snapshot")]
#[command(about = "Build and verify the offline proper-name snapshot artifacts")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Build {
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        out: PathBuf,
    },
    Verify {
        #[arg(long)]
        out: PathBuf,
    },
    #[command(name = "export-report")]
    ExportReport {
        #[arg(long)]
        out: PathBuf,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Build { config, out } => {
            let result = build_snapshot(&config, &out)?;
            println!("snapshot.sqlite: {}", result.sqlite_path.display());
            println!("parquet/: {}", result.parquet_dir.display());
            println!("image_store/: {}", result.image_store_dir.display());
            for (table, count) in result.row_counts {
                println!("{table}: {count}");
            }
            if !result.warnings.is_empty() {
                println!("warnings:");
                for warning in result.warnings {
                    println!("- {warning}");
                }
            }
        }
        Commands::Verify { out } => {
            let checks = verify_output_dir(&out)?;
            println!("verified {}", out.display());
            for line in checks {
                println!("{line}");
            }
        }
        Commands::ExportReport { out } => {
            regenerate_reports(&out)?;
            println!("reports regenerated in {}", out.display());
        }
    }
    Ok(())
}
