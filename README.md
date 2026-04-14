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

## Usage

1. **Source Path:** Enter the directory containing your music.
2. **Format Selection:** Choose specific tiers (e.g., `legacy, universal`) or type `all`.
3. **Output Path:** Specify where to save your optimized library. The tool will automatically create sub-folders for each preset.
4. **Core Count:** Select how many CPU cores to dedicate to the task. (Recommended: Total Cores - 1).

---

## Technical Architecture

1. **Demux & Decode:** Raw packets are extracted and decoded into `f32` planar audio.
2. **Synchronous Resampling:** If the source sample rate differs from the target, a high-order polynomial FFT resampler realigns the audio curve.
3. **Quantization:** Audio is scaled from floating-point to fixed-point integers (16-bit or 24-bit) using dither-free clamping.
4. **Muxing:** The resulting stream is wrapped into a Pioneer-compliant container (WAV/FLAC) with correct header specs.

### License

[FOUL (Fair Open-Use License)](LICENSE-FOUL)
