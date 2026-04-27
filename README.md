# Pioneer DJ Library Converter

A **pure Rust** audio transcoding engine designed to optimize music libraries for the entire spectrum of Pioneer DJ hardware.

---

## Performance Profiles

| Profile       | Format | Specs            | Target Hardware                                    |
| :------------ | :----- | :--------------- | :------------------------------------------------- |
| **Flagship**  | FLAC   | 24-bit / 96kHz   | CDJ-3000, Opus Quad                                |
| **Standard**  | AIFF   | 24-bit / 48kHz   | CDJ-2000NXS2, XDJ-XZ                               |
| **Legacy**    | WAV    | 16-bit / 44.1kHz | CDJ-850, CDJ-350, XDJ-700                          |
| **Universal** | WAV    | 16-bit / 44.1kHz | **Bulletproof:** Works on every Pioneer USB player |

---

## Installation

Ensure you have the [Rust toolchain](https://rustup.rs/) installed.

1. Clone the repository.
2. Build and Run the optimized release binary:

   ```bash
   cargo run --release
   ```

---

### 📖 Usage

The Pioneer DJ Library Converter supports both an interactive terminal interface (TUI) and a high-speed command-line interface (CLI).

#### **Interactive Mode (TUI)**

If you run the program without arguments, it will guide you through a set of interactive questions:

1. **Source Path:** Enter the directory containing your music (e.g., `D:\Music\originals`).
2. **Format Selection:** Choose specific hardware tiers using the arrow keys and spacebar (e.g., `flagship`, `legacy`).
3. **Output Path:** Specify where to save your optimized library. The tool will automatically create sub-folders for each preset.
4. **Core Count:** Select how many CPU cores to dedicate to the task. (Recommended: Total Cores - 1).

#### **Command-Line Interface (CLI)**

you can bypass the questions by passing arguments directly.

| Flag | Full Name   | Description                                                                     |
| :--- | :---------- | :------------------------------------------------------------------------------ |
| `-i` | `--input`   | Path to your source music folder                                                |
| `-o` | `--output`  | Path for the converted output folder                                            |
| `-p` | `--presets` | Comma-separated list of formats (`flagship`, `standard`, `legacy`, `universal`) |
| `-c` | `--cores`   | Number of CPU cores to use for parallel processing                              |

**Example Command:**

```bash
cargo run --release -- -i "D:\Music\originals" -o "D:\Music\originals_format" -p flagship,standard -c 10
```

## Technical Architecture

1. **Demux & Decode:** Raw packets are extracted and decoded into `f32` planar audio.
2. **Synchronous Resampling:** If the source sample rate differs from the target, a high-order polynomial FFT resampler realigns the audio curve.
3. **Quantization:** Audio is scaled from floating-point to fixed-point integers (16-bit or 24-bit) using dither-free clamping.
4. **Muxing:** The resulting stream is wrapped into a Pioneer-compliant container (WAV/FLAC) with correct header specs.

### License

[FOUL (Fair Open-Use License)](LICENSE-FOUL)
