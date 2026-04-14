use pioneer_converter::{check_audio_quality, get_presets, run_conversion};
use std::path::{Path, PathBuf};
use tokio::task::JoinSet;
use walkdir::WalkDir;

// TUI Tooling
use console::{Emoji, style};
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use inquire::{CustomType, MultiSelect, Text, validator::Validation};

static CHECK: Emoji<'_, '_> = Emoji("✅ ", "");
static WARN: Emoji<'_, '_> = Emoji("⚠️  ", "");

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
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

    // --- Interactive Inputs ---
    let input_dir = Text::new("Where is your source music folder?")
        .with_default("./input")
        .with_validator(|val: &str| {
            if Path::new(val).is_dir() {
                Ok(Validation::Valid)
            } else {
                Ok(Validation::Invalid("Directory not found!".into()))
            }
        })
        .prompt()?;

    let all_presets = get_presets();
    let selected_profiles =
        MultiSelect::new("Target hardware tiers:", all_presets.clone()).prompt()?;

    let default_output = if selected_profiles.len() == 1 {
        format!("./converted_{}", selected_profiles[0].name)
    } else {
        "./converted".to_string()
    };

    let output_base = Text::new("Output folder:")
        .with_default(&default_output)
        .prompt()?;

    let max_cores = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    let num_cores = CustomType::<usize>::new("Simultaneous tracks (CPU Cores):")
        .with_default(if max_cores > 1 { max_cores - 1 } else { 1 })
        .prompt()?;

    let m = MultiProgress::new();
    let mut final_warning_count = 0;

    // --- Execution ---
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

        // Updated Template: {msg} will display the current filename
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

            // 1. Real-time Quality Check
            if let Some(msg) = check_audio_quality(&path, &profile) {
                // We use m.println so the warning appears ABOVE the progress bar
                m.println(format!(
                    "  {} {}: {}",
                    style(WARN).yellow(),
                    style(&stem).dim(),
                    style(msg).yellow()
                ))?;
                final_warning_count += 1;
            }

            // 2. Update the "Current File" display
            pb.set_message(format!(
                "  {} Processing: {}",
                style("↳").dim(),
                style(&stem).blue()
            ));

            let path_clone = path.clone();
            let output_clone = output_file.clone();
            let profile_clone = profile.clone();
            let stem_clone = stem.clone();

            // Throttling Logic
            while set.len() >= num_cores {
                if let Some(res) = set.join_next().await {
                    let _ = res.unwrap();
                    completed += 1;
                    pb.set_position(completed);
                }
            }

            set.spawn(async move {
                let res = tokio::task::spawn_blocking(move || {
                    run_conversion(path_clone, output_clone, &profile_clone)
                })
                .await
                .unwrap();
                (stem_clone, res)
            });
        }

        // Final Drain
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
