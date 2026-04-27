use audioadapter_buffers::direct::InterleavedSlice;
use hound::{SampleFormat, WavSpec, WavWriter};
use id3::frame::{Picture as Id3Picture, PictureType as Id3PictureType};
use id3::{Tag, TagLike, Version};
use metaflac::{Block, Tag as FlacTag};
use rubato::{Fft, FixedSync, Indexing, Resampler};
use std::fs::File;
use std::path::{Path, PathBuf};
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{CODEC_TYPE_AAC, CODEC_TYPE_MP3, CodecParameters, DecoderOptions};
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::meta::StandardTagKey;
use symphonia::core::probe::Hint;

impl std::fmt::Display for ConversionProfile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} ({} - {}Hz)",
            self.name, self.ext, self.target_sample_rate
        )
    }
}

#[derive(Clone)]
pub struct ConversionProfile {
    pub name: &'static str,
    pub ext: &'static str,
    pub target_sample_rate: u32,
    pub target_bit_depth: u32,
}

pub fn get_presets() -> Vec<ConversionProfile> {
    vec![
        ConversionProfile {
            name: "flagship",
            ext: "flac",
            target_sample_rate: 96000,
            target_bit_depth: 24,
        },
        ConversionProfile {
            name: "standard",
            ext: "aiff",
            target_sample_rate: 48000,
            target_bit_depth: 16,
        },
        ConversionProfile {
            name: "legacy",
            ext: "wav",
            target_sample_rate: 44100,
            target_bit_depth: 16,
        },
        ConversionProfile {
            name: "universal",
            ext: "wav",
            target_sample_rate: 44100,
            target_bit_depth: 16,
        },
    ]
}

pub fn check_audio_quality(input_path: &Path, profile: &ConversionProfile) -> Option<String> {
    let file = match File::open(input_path) {
        Ok(f) => f,
        Err(_) => return Some("Could not open file for quality check".to_string()),
    };

    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();

    if let Some(ext) = input_path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }

    let probe_res = symphonia::default::get_probe().format(
        &hint,
        mss,
        &FormatOptions::default(),
        &MetadataOptions::default(),
    );

    if let Ok(probed) = probe_res {
        let format = probed.format;

        if let Some(track) = format.default_track() {
            let params: &CodecParameters = &track.codec_params;

            let source_hz = params.sample_rate.unwrap_or(44100);
            let source_bits = params.bits_per_sample.unwrap_or(16);

            if source_hz < profile.target_sample_rate {
                return Some(format!(
                    "UPSAMPLE WARNING: Source is {}kHz. Converting to {}kHz for {} wastes USB space.",
                    source_hz as f32 / 1000.0,
                    profile.target_sample_rate as f32 / 1000.0,
                    profile.name.to_uppercase()
                ));
            }

            if source_bits < profile.target_bit_depth {
                return Some(format!(
                    "BIT-DEPTH WARNING: Source is {}-bit. Padding to {}-bit for {} creates unnecessarily large files.",
                    source_bits,
                    profile.target_bit_depth,
                    profile.name.to_uppercase()
                ));
            }

            if params.codec == CODEC_TYPE_MP3 || params.codec == CODEC_TYPE_AAC {
                return Some(format!(
                    "LOSSY WARNING: Source is a compressed lossy format. Converting to {} will not recover lost audio data.",
                    profile.ext.to_uppercase()
                ));
            }
        }
    }
    None
}
fn quantize_and_write<W: std::io::Write + std::io::Seek>(
    writer: &mut WavWriter<W>,
    sample: f32,
    bit_depth: u32,
) -> Result<(), hound::Error> {
    if bit_depth == 16 {
        let scaled = (sample * i16::MAX as f32).clamp(i16::MIN as f32, i16::MAX as f32) as i16;
        writer.write_sample(scaled)
    } else {
        let max_24 = 8_388_607.0;
        let min_24 = -8_388_608.0;
        let scaled = (sample * max_24).clamp(min_24, max_24) as i32;
        writer.write_sample(scaled)
    }
}

pub fn run_conversion(
    input: PathBuf,
    output: PathBuf,
    profile: &ConversionProfile,
    force_upsampling: bool,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let file = File::open(&input)?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = input.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }

    let mut probed = symphonia::default::get_probe().format(
        &hint,
        mss,
        &FormatOptions::default(),
        &MetadataOptions::default(),
    )?;

    let mut format = probed.format;

    let track = format
        .default_track()
        .ok_or("No default audio track found")?;

    let params = &track.codec_params;
    let source_hz = params.sample_rate.unwrap_or(44100);
    let source_bits = params.bits_per_sample.unwrap_or(16);
    let channels = params
        .channels
        .unwrap_or(
            symphonia::core::audio::Channels::FRONT_LEFT
                | symphonia::core::audio::Channels::FRONT_RIGHT,
        )
        .count();
    let track_id = track.id;

    let mut decoder =
        symphonia::default::get_codecs().make(&track.codec_params, &DecoderOptions::default())?;

    let mut target_hz = profile.target_sample_rate;
    let mut target_bits = profile.target_bit_depth;

    if !force_upsampling {
        if source_hz < target_hz {
            target_hz = source_hz;
        }
        if source_bits < target_bits {
            target_bits = source_bits;
        }
    }

    let spec = WavSpec {
        channels: channels as u16,
        sample_rate: target_hz,
        bits_per_sample: target_bits as u16,
        sample_format: SampleFormat::Int,
    };

    let mut writer = WavWriter::create(&output, spec)?;
    let needs_resampling = source_hz != target_hz;

    let mut resampler = if needs_resampling {
        Some(Fft::<f32>::new(
            source_hz as usize,
            profile.target_sample_rate as usize,
            1024,
            2,
            channels,
            FixedSync::Both,
        )?)
    } else {
        None
    };

    let mut sample_buf = None;
    let mut input_accumulator: Vec<f32> = Vec::new();
    let mut outdata = vec![0.0; channels * 4096];

    loop {
        let packet = match format.next_packet() {
            Ok(packet) => packet,
            Err(symphonia::core::errors::Error::IoError(err))
                if err.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break;
            }
            Err(e) => return Err(e.into()),
        };

        if packet.track_id() != track_id {
            continue;
        }

        match decoder.decode(&packet) {
            Ok(audio_buf) => {
                if sample_buf.is_none() {
                    sample_buf = Some(SampleBuffer::<f32>::new(
                        audio_buf.capacity() as u64,
                        *audio_buf.spec(),
                    ));
                }

                if let Some(buf) = &mut sample_buf {
                    buf.copy_interleaved_ref(audio_buf);
                    let floats = buf.samples();

                    if !needs_resampling {
                        for &sample in floats {
                            quantize_and_write(&mut writer, sample, profile.target_bit_depth)?;
                        }
                    } else {
                        input_accumulator.extend_from_slice(floats);

                        let res = resampler.as_mut().unwrap();
                        let mut input_frames_next = res.input_frames_next();

                        while (input_accumulator.len() / channels) >= input_frames_next {
                            let chunk_samples = input_frames_next * channels;
                            let input_adapter = InterleavedSlice::new(
                                &input_accumulator[..chunk_samples],
                                channels,
                                input_frames_next,
                            )?;

                            let out_frames_max = res.output_frames_max();
                            if outdata.len() < out_frames_max * channels {
                                outdata.resize(out_frames_max * channels, 0.0);
                            }

                            let mut output_adapter =
                                InterleavedSlice::new_mut(&mut outdata, channels, out_frames_max)?;

                            let indexing = Indexing {
                                input_offset: 0,
                                output_offset: 0,
                                active_channels_mask: None,
                                partial_len: None,
                            };

                            let (frames_read, frames_written) = res.process_into_buffer(
                                &input_adapter,
                                &mut output_adapter,
                                Some(&indexing),
                            )?;

                            for &sample in outdata.iter().take(frames_written * channels) {
                                quantize_and_write(&mut writer, sample, profile.target_bit_depth)?;
                            }

                            input_accumulator.drain(0..(frames_read * channels));
                            input_frames_next = res.input_frames_next();
                        }
                    }
                }
            }
            Err(symphonia::core::errors::Error::DecodeError(_)) => continue,
            Err(e) => return Err(e.into()),
        }
    }

    if needs_resampling && !input_accumulator.is_empty() {
        let res = resampler.as_mut().unwrap();
        let frames_left = input_accumulator.len() / channels;
        let input_adapter = InterleavedSlice::new(&input_accumulator, channels, frames_left)?;

        let out_frames_max = res.output_frames_max();
        if outdata.len() < out_frames_max * channels {
            outdata.resize(out_frames_max * channels, 0.0);
        }

        let mut output_adapter = InterleavedSlice::new_mut(&mut outdata, channels, out_frames_max)?;

        let indexing = Indexing {
            input_offset: 0,
            output_offset: 0,
            active_channels_mask: None,
            partial_len: Some(frames_left),
        };

        let (_, frames_written) =
            res.process_into_buffer(&input_adapter, &mut output_adapter, Some(&indexing))?;

        for &sample in outdata.iter().take(frames_written * channels) {
            quantize_and_write(&mut writer, sample, profile.target_bit_depth)?;
        }
    }

    writer.finalize()?;

    if let Some(mut metadata) = probed.metadata.get()
        && let Some(metadata) = metadata.skip_to_latest()
    {
        transfer_metadata(metadata, &output, profile)?;
    }

    Ok(())
}

fn transfer_metadata(
    metadata: &symphonia::core::meta::MetadataRevision,
    output_path: &Path,
    profile: &ConversionProfile,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    match profile.ext {
        "wav" | "aiff" => {
            let mut tag = Tag::new();

            for sym_tag in metadata.tags() {
                if let Some(key) = sym_tag.std_key {
                    match key {
                        StandardTagKey::Album => tag.set_album(sym_tag.value.to_string()),
                        StandardTagKey::Artist => tag.set_artist(sym_tag.value.to_string()),
                        StandardTagKey::TrackTitle => tag.set_title(sym_tag.value.to_string()),
                        StandardTagKey::Genre => tag.set_genre(sym_tag.value.to_string()),
                        StandardTagKey::TrackNumber => {
                            let val = sym_tag.value.to_string().parse().unwrap_or(0);
                            tag.set_track(val);
                        }
                        _ => {}
                    }
                }
            }

            for visual in metadata.visuals() {
                tag.add_frame(Id3Picture {
                    mime_type: visual.media_type.to_string(),
                    picture_type: Id3PictureType::CoverFront,
                    description: "Front Cover".to_string(),
                    data: visual.data.to_vec(),
                });
            }
            tag.write_to_path(output_path, Version::Id3v24)?;
        }
        "flac" => {
            let mut flac_tag =
                FlacTag::read_from_path(output_path).unwrap_or_else(|_| FlacTag::new());

            for sym_tag in metadata.tags() {
                let key = sym_tag
                    .std_key
                    .map(|k| format!("{:?}", k))
                    .unwrap_or_else(|| sym_tag.key.clone());
                flac_tag
                    .vorbis_comments_mut()
                    .set(key, vec![sym_tag.value.to_string()]);
            }

            for visual in metadata.visuals() {
                let flac_pic = metaflac::block::Picture {
                    picture_type: metaflac::block::PictureType::CoverFront,
                    mime_type: visual.media_type.to_string(),
                    description: "Front Cover".to_string(),
                    width: visual.dimensions.map(|d| d.width).unwrap_or(0),
                    height: visual.dimensions.map(|d| d.height).unwrap_or(0),
                    depth: 24,
                    num_colors: 0,
                    data: visual.data.to_vec(),
                };
                flac_tag.push_block(Block::Picture(flac_pic));
            }
            flac_tag
                .write_to_path(output_path)
                .map_err(|_| "Failed to save FLAC metadata")?;
        }
        _ => {}
    }

    Ok(())
}
