use clap::Parser;
use console::{Emoji, style};
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use inquire::{Confirm, CustomType, MultiSelect, Text, validator::Validation};
use pioneer_converter::{check_audio_quality, get_presets, run_conversion};
use std::path::{Path, PathBuf};
use tokio::task::JoinSet;
use walkdir::WalkDir;

static CHECK: Emoji<'_, '_> = Emoji("✅ ", "");
static WARN: Emoji<'_, '_> = Emoji("⚠️  ", "");

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Path to source music folder
    #[arg(short, long)]
    input: Option<PathBuf>,

    /// Path to output folder
    #[arg(short, long)]
    output: Option<PathBuf>,

    /// Comma-separated list of presets (flagship, standard, legacy, universal)
    #[arg(short, long, value_delimiter = ',')]
    presets: Option<Vec<String>>,

    /// Number of CPU cores to use
    #[arg(short, long)]
    cores: Option<usize>,

    /// Force upsampling even if source quality is lower than preset
    #[arg(short, long, default_value = "false")]
    force_upsampling: Option<bool>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    println!(
        "{}",
        style("==========================================================").cyan()
    );
    println!(
        "{}",
        style("     PIONEER DJ LIBRARY CONVERTER (RUST TUI) ")
            .bold()
            .cyan()
    );
    println!(
        "{}",
        style("==========================================================\n").cyan()
    );

    let input_dir: String = match args.input {
        Some(p) if p.is_dir() => p.to_string_lossy().into_owned(),
        _ => Text::new("Where is your source music folder?")
            .with_default("./input")
            .with_validator(|val: &str| {
                if Path::new(val).is_dir() {
                    Ok(Validation::Valid)
                } else {
                    Ok(Validation::Invalid("Directory not found!".into()))
                }
            })
            .prompt()?,
    };

    let all_available = get_presets();
    let selected_profiles = match args.presets {
        Some(p) => {
            let mut filtered = Vec::new();
            for name in p {
                if let Some(prof) = all_available
                    .iter()
                    .find(|ap| ap.name == name.to_lowercase())
                {
                    filtered.push(prof.clone());
                } else {
                    println!("{} Unknown preset skipped: {}", style("!").yellow(), name);
                }
            }
            if filtered.is_empty() {
                return Err("No valid presets provided.".into());
            }
            filtered
        }
        None => MultiSelect::new("Target hardware tiers:", all_available).prompt()?,
    };

    let force_upsampling = match args.force_upsampling {
        Some(b) => b,
        None => Confirm::new("Force upsampling for low-quality files (not recommended)?")
            .with_default(false)
            .with_help_message("If no, low-res files will keep their native rate to save space.")
            .prompt()?,
    };

    let output_base: String = match args.output {
        Some(p) => p.to_string_lossy().into_owned(),
        None => {
            let default_out = if selected_profiles.len() == 1 {
                format!("./converted_{}", selected_profiles[0].name)
            } else {
                "./converted".to_string()
            };

            Text::new("Output folder:")
                .with_default(&default_out)
                .prompt()?
        }
    };

    let max_cores = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    let num_cores = match args.cores {
        Some(c) => c.clamp(1, max_cores),
        None => CustomType::<usize>::new("Simultaneous tracks (CPU Cores):")
            .with_default(if max_cores > 1 { max_cores - 1 } else { 1 })
            .prompt()?,
    };

    let m = MultiProgress::new();
    let mut final_warning_count = 0;

    for profile in selected_profiles {
        let dest_dir = Path::new(&output_base).join(profile.name);
        tokio::fs::create_dir_all(&dest_dir).await?;

        m.println(format!(
            "\n{} Tier: {}",
            style("▶").green(),
            style(profile.name.to_uppercase()).bold().yellow()
        ))?;

        let files: Vec<PathBuf> = WalkDir::new(&input_dir)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_file())
            .map(|e| e.path().to_path_buf())
            .collect();

        if files.is_empty() {
            continue;
        }

        let pb = m.add(ProgressBar::new(files.len() as u64));
        pb.set_style(
            ProgressStyle::with_template(
                "{spinner:.green} [{bar:40.cyan/blue}] {pos}/{len}\n{msg}",
            )?
            .progress_chars("#>-"),
        );

        let mut set = JoinSet::new();
        let mut completed = 0;

        for path in files {
            let stem = path.file_stem().unwrap().to_str().unwrap().to_string();
            let output_file = dest_dir.join(format!("{}.{}", stem, profile.ext));

            if let Some(msg) = check_audio_quality(&path, &profile) {
                m.println(format!(
                    "  {} {}: {}",
                    style(WARN).yellow(),
                    style(&stem).dim(),
                    style(msg).yellow()
                ))?;
                final_warning_count += 1;
            }

            pb.set_message(format!(
                "  {} Processing: {}",
                style("↳").dim(),
                style(&stem).blue()
            ));

            let path_clone = path.clone();
            let output_clone = output_file.clone();
            let profile_clone = profile.clone();
            let stem_clone = stem.clone();

            while set.len() >= num_cores {
                if let Some(res) = set.join_next().await {
                    let _ = res.unwrap();
                    completed += 1;
                    pb.set_position(completed);
                }
            }

            set.spawn(async move {
                let res = tokio::task::spawn_blocking(move || {
                    run_conversion(path_clone, output_clone, &profile_clone, force_upsampling)
                })
                .await
                .unwrap();
                (stem_clone, res)
            });
        }

        while let Some(res) = set.join_next().await {
            let _ = res.unwrap();
            completed += 1;
            pb.set_position(completed);
        }

        pb.finish_with_message(format!(
            "  {} Finished {}",
            style("✔").green(),
            profile.name
        ));
    }

    println!(
        "\n{} {}",
        CHECK,
        style("Library conversion complete!").bold().green()
    );
    if final_warning_count > 0 {
        println!(
            "{}",
            style(format!(
                "Processed with {} quality warnings.",
                final_warning_count
            ))
            .dim()
        );
    }

    Ok(())
}
