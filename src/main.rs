use clap::Parser;
use console::{Emoji, style};
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use inquire::{Confirm, MultiSelect, Text};
use std::path::{Path, PathBuf};
use tokio::task::JoinSet;
use walkdir::WalkDir;

// Import profile/presets regardless of feature, but gate engine logic
use pioneer_converter::{ConversionProfile, get_presets};

#[cfg(feature = "ffmpeg")]
static FFMPEG_BINARY: &[u8] = if cfg!(windows) {
    include_bytes!("../bin/ffmpeg-windows.exe")
} else {
    include_bytes!("../bin/ffmpeg-linux")
};

#[allow(dead_code)]
static CHECK: Emoji<'_, '_> = Emoji("✅ ", "");
#[allow(dead_code)]
static WARN: Emoji<'_, '_> = Emoji("⚠️  ", "");

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[arg(short, long)]
    input: Option<PathBuf>,
    #[arg(short, long)]
    output: Option<PathBuf>,
    #[arg(short, long, value_delimiter = ',')]
    presets: Option<Vec<String>>,
    #[arg(short, long)]
    cores: Option<usize>,
    #[arg(short, long, default_value = "false")]
    force_upsampling: Option<bool>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let args = Args::parse();
    print_header();

    setup_environment()?;

    let input_dir = get_input_directory(&args)?;
    let profiles = get_selected_profiles(&args)?;
    let force_up = get_force_upsampling(&args)?;
    let output_base = get_output_directory(&args, &profiles)?;
    let num_cores = get_core_count(&args);

    let m = MultiProgress::new();
    let mut total_warnings = 0;

    for profile in profiles {
        total_warnings +=
            process_tier(&m, &input_dir, &output_base, &profile, num_cores, force_up).await?;
    }

    print_summary(total_warnings);
    Ok(())
}

async fn process_tier(
    m: &MultiProgress,
    input_dir: &Path,
    output_base: &Path,
    profile: &ConversionProfile,
    num_cores: usize,
    force_up: bool,
) -> Result<usize, Box<dyn std::error::Error + Send + Sync>> {
    let dest_dir = output_base.join(profile.name);
    tokio::fs::create_dir_all(&dest_dir).await?;

    m.println(format!(
        "\n{} Tier: {}",
        style("▶").green(),
        style(profile.name.to_uppercase()).bold().yellow()
    ))?;

    let files = collect_audio_files(input_dir);
    if files.is_empty() {
        m.println(style("  No compatible files found.").dim().to_string())?;
        return Ok(0);
    }

    let pb = m.add(create_progress_bar(files.len()));
    let mut set = JoinSet::new();
    let mut completed = 0;
    #[allow(unused_mut)]
    let mut warnings = 0;

    for path in files {
        let stem = path.file_stem().unwrap().to_string_lossy().to_string();
        let output_file = dest_dir.join(format!("{}.{}", stem, profile.ext));

        #[cfg(feature = "pure-rust")]
        {
            use pioneer_converter::rust_engine::check_audio_quality;

            if let Some(msg) = check_audio_quality(&path, profile) {
                m.println(format!(
                    "  {} {}: {}",
                    style(WARN).yellow(),
                    style(&stem).dim(),
                    style(msg).yellow()
                ))?;
                warnings += 1;
            }
        }

        let pb_clone = pb.clone();
        let (p_in, p_out, p_prof) = (path.clone(), output_file.clone(), profile.clone());

        while set.len() >= num_cores {
            if let Some(res) = set.join_next().await {
                handle_worker_result(m, &pb, res, &mut completed);
            }
        }

        set.spawn(
            async move { run_conversion_logic(p_in, p_out, p_prof, force_up, pb_clone).await },
        );
    }

    while let Some(res) = set.join_next().await {
        handle_worker_result(m, &pb, res, &mut completed);
    }

    pb.finish_with_message(format!("{} Completed {}", style("✔").green(), profile.name));
    Ok(warnings)
}

async fn run_conversion_logic(
    input: PathBuf,
    output: PathBuf,
    profile: ConversionProfile,
    force: bool,
    pb: ProgressBar,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    #[cfg(feature = "ffmpeg")]
    {
        run_ffmpeg_engine(input, output, profile, pb, force).await
    }

    #[cfg(all(not(feature = "ffmpeg"), feature = "pure-rust"))]
    {
        use pioneer_converter::rust_engine::run_conversion;

        let _ = pb;
        tokio::task::spawn_blocking(move || run_conversion(input, output, &profile, force)).await?
    }
}

#[cfg(feature = "ffmpeg")]
async fn run_ffmpeg_engine(
    input: PathBuf,
    output: PathBuf,
    profile: ConversionProfile,
    pb: ProgressBar,
    force_upsampling: bool,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
    use tokio::process::Command;

    let ffmpeg_path = get_or_extract_ffmpeg()?;

    let abs_input = input
        .canonicalize()
        .map_err(|e| format!("Input path error: {} ({:?})", e, input))?;

    let source_hz = get_source_sample_rate(&input).await?;

    let mut target_hz = profile.target_sample_rate;

    if !force_upsampling && source_hz < target_hz {
        target_hz = source_hz;
    }

    let abs_output = if output.exists() {
        output.canonicalize()?
    } else {
        let parent = output.parent().unwrap().canonicalize()?;
        parent.join(output.file_name().unwrap())
    };

    let mut cmd = Command::new(ffmpeg_path);
    cmd.arg("-y")
        .arg("-loglevel")
        .arg("level")
        .arg("-i")
        .arg(clean_path(&abs_input))
        .arg("-progress")
        .arg("pipe:1")
        .arg("-map_metadata")
        .arg("0")
        .arg("-hide_banner")
        .arg("-stats")
        .arg("-ar")
        .arg(target_hz.to_string())
        .args(get_ffmpeg_codec_args(&profile))
        .arg(clean_path(&abs_output));

    let mut child = cmd
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()?;

    let stem = input.file_stem().unwrap().to_string_lossy();

    let progress_pb = pb.clone();
    let progress_stem = stem.to_string();

    let stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let mut reader = BufReader::new(stdout).lines();

    tokio::spawn(async move {
        // Rust sollte den Typ nun durch AsyncBufReadExt korrekt erkennen
        while let Ok(Some(line)) = reader.next_line().await {
            if line.starts_with("out_time=") {
                let time = line.replace("out_time=", "").trim().to_string();
                progress_pb.set_message(format!(
                    "{} | {}",
                    style(time).dim(),
                    style(&progress_stem).blue()
                ));
            }
        }
    });

    let status = child.wait().await?;

    if !status.success() {
        let mut err_msg = String::new();
        stderr.read_to_string(&mut err_msg).await?;
        return Err(format!("FFmpeg Error: {}", err_msg.trim()).into());
    }

    Ok(())
}

#[cfg(feature = "ffmpeg")]
fn clean_path(path: &Path) -> String {
    let path_str = path.to_string_lossy();
    path_str
        .strip_prefix(r"\\?\")
        .unwrap_or(&path_str)
        .to_string()
}

#[cfg(feature = "ffmpeg")]
async fn get_source_sample_rate(
    input: &Path,
) -> Result<u32, Box<dyn std::error::Error + Send + Sync>> {
    let ffmpeg_path = get_or_extract_ffmpeg()?;
    let output = std::process::Command::new(ffmpeg_path)
        .arg("-i")
        .arg(input)
        .output()?;
    let info = String::from_utf8_lossy(&output.stderr);

    if let Some(idx) = info.find(" Hz") {
        let start = info[..idx].rfind(' ').unwrap_or(0);
        let hz_str = info[start..idx].trim();
        if let Ok(hz) = hz_str.parse::<u32>() {
            return Ok(hz);
        }
    }
    Ok(44100)
}

#[cfg(feature = "ffmpeg")]
fn get_ffmpeg_codec_args(profile: &ConversionProfile) -> Vec<String> {
    match (profile.ext, profile.target_bit_depth) {
        ("flac", _) => vec!["-c:a".to_string(), "flac".to_string()],
        ("wav", 16) => vec![
            "-c:a".to_string(),
            "pcm_s16le".to_string(),
            "-write_id3v2".to_string(),
            "1".to_string(),
        ],
        ("wav", 24) => vec![
            "-c:a".to_string(),
            "pcm_s24le".to_string(),
            "-write_id3v2".to_string(),
            "1".to_string(),
        ],
        ("aiff", 16) => vec![
            "-c:a".to_string(),
            "pcm_s16be".to_string(),
            "-write_id3v2".to_string(),
            "1".to_string(),
        ],
        ("aiff", 24) => vec![
            "-c:a".to_string(),
            "pcm_s24be".to_string(),
            "-write_id3v2".to_string(),
            "1".to_string(),
        ],
        _ => vec!["-c:a".to_string(), "pcm_s16le".to_string()],
    }
}

#[cfg(feature = "ffmpeg")]
fn get_or_extract_ffmpeg() -> Result<PathBuf, Box<dyn std::error::Error + Send + Sync>> {
    use std::io::Write;
    let mut temp_path = std::env::temp_dir();
    let exe_name = if cfg!(windows) {
        "ffmpeg_windows.exe"
    } else {
        "ffmpeg_linux"
    };
    temp_path.push(exe_name);

    if !temp_path.exists() {
        let mut file = std::fs::File::create(&temp_path)
            .map_err(|e| format!("Could not create temp file at {:?}: {}", temp_path, e))?;
        file.write_all(FFMPEG_BINARY)?;
        file.flush()?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = file.metadata()?.permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&temp_path, perms)?;
        }
    }

    if !temp_path.exists() {
        return Err(format!(
            "FFmpeg binary missing after extraction attempt at: {:?}",
            temp_path
        )
        .into());
    }

    Ok(temp_path)
}

fn setup_environment() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    #[cfg(feature = "ffmpeg")]
    {
        let ffmpeg_path = get_or_extract_ffmpeg()?;

        let output = std::process::Command::new(&ffmpeg_path)
            .arg("-version")
            .output()
            .map_err(|e| format!("Failed to execute FFmpeg check: {}", e))?;

        if output.status.success() {
            let version_info = String::from_utf8_lossy(&output.stdout);
            let first_line = version_info.lines().next().unwrap_or("Unknown version");

            println!(
                "{} {} verified: {}",
                CHECK,
                style("FFmpeg Binary").bold().green(),
                style(first_line).dim()
            );
        } else {
            return Err(format!(
                "FFmpeg binary extracted but failed to run. (Status: {:?})",
                output.status
            )
            .into());
        }
    }

    Ok(())
}

fn collect_audio_files(dir: &Path) -> Vec<PathBuf> {
    WalkDir::new(dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_file())
        .filter(|e| {
            matches!(
                e.path().extension().and_then(|s| s.to_str()),
                Some("wav" | "flac" | "aiff" | "mp3" | "m4a")
            )
        })
        .map(|e| e.path().to_path_buf())
        .collect()
}

fn handle_worker_result(
    m: &MultiProgress,
    pb: &ProgressBar,
    res: Result<Result<(), Box<dyn std::error::Error + Send + Sync>>, tokio::task::JoinError>,
    completed: &mut usize,
) {
    if let Ok(Err(e)) = res {
        let _ = m.println(format!(
            "  {} {}: {}",
            style("✘").red(),
            style("Error").bold().red(),
            style(e).yellow()
        ));
    }
    *completed += 1;
    pb.set_position(*completed as u64);
}

fn print_header() {
    println!(
        "{}",
        style("==========================================================").cyan()
    );
    println!(
        "{}",
        style("     PIONEER DJ LIBRARY CONVERTER ").bold().cyan()
    );
    println!(
        "{}\n",
        style("==========================================================").cyan()
    );
}

fn get_input_directory(args: &Args) -> Result<PathBuf, Box<dyn std::error::Error + Send + Sync>> {
    if let Some(ref p) = args.input
        && p.is_dir()
    {
        return Ok(p.clone());
    }
    Ok(PathBuf::from(
        Text::new("Source folder:")
            .with_default("./input")
            .prompt()?,
    ))
}

fn get_selected_profiles(
    args: &Args,
) -> Result<Vec<ConversionProfile>, Box<dyn std::error::Error + Send + Sync>> {
    let all = get_presets();
    if let Some(ref p) = args.presets {
        return Ok(all
            .into_iter()
            .filter(|ap| p.contains(&ap.name.to_string()))
            .collect());
    }
    Ok(MultiSelect::new("Target tiers:", all).prompt()?)
}

fn get_force_upsampling(args: &Args) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
    if let Some(b) = args.force_upsampling {
        return Ok(b);
    }
    Ok(Confirm::new("Force upsampling?")
        .with_default(false)
        .prompt()?)
}

fn get_output_directory(
    args: &Args,
    profiles: &[ConversionProfile],
) -> Result<PathBuf, Box<dyn std::error::Error + Send + Sync>> {
    if let Some(ref p) = args.output {
        return Ok(p.clone());
    }
    let default = if profiles.len() == 1 {
        format!("./converted_{}", profiles[0].name)
    } else {
        "./converted".to_string()
    };
    Ok(PathBuf::from(
        Text::new("Output folder:")
            .with_default(&default)
            .prompt()?,
    ))
}

fn get_core_count(args: &Args) -> usize {
    let max = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    args.cores.unwrap_or(if max > 1 { max - 1 } else { 1 })
}

fn create_progress_bar(len: usize) -> ProgressBar {
    let pb = ProgressBar::new(len as u64);
    pb.set_style(
        ProgressStyle::with_template("{spinner:.green} [{bar:40.cyan/blue}] {pos}/{len} {msg}")
            .unwrap()
            .progress_chars("#>-"),
    );
    pb
}

fn print_summary(warnings: usize) {
    println!(
        "\n{} {}",
        CHECK,
        style("Library conversion complete!").bold().green()
    );
    if warnings > 0 {
        println!(
            "{}",
            style(format!("Processed with {} warnings.", warnings)).dim()
        );
    }
}
