use std::{
    env,
    error::Error,
    fmt,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
    thread,
    time::Duration,
};

use cpal::{
    traits::{DeviceTrait, HostTrait, StreamTrait},
    FromSample, Sample, SampleFormat, SizedSample,
};

const DURATION_SECONDS: usize = 2;
const USAGE: &str =
    "Usage: synthesizer <note>\nExamples: synthesizer c, synthesizer f#, synthesizer c4";

#[derive(Clone, Copy)]
struct ToneParameters {
    channels: usize,
    sample_rate: usize,
    total_frames: usize,
    frequency: f32,
}

#[derive(Debug)]
struct InvalidNote(String);

impl fmt::Display for InvalidNote {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "invalid note '{}'; use a note name such as c, f#, bb, or c4",
            self.0
        )
    }
}

impl Error for InvalidNote {}

fn note_frequency(note: &str) -> Result<f32, InvalidNote> {
    let note = note.trim().to_ascii_lowercase();
    let split_at = note
        .find(|character: char| character.is_ascii_digit() || character == '-')
        .unwrap_or(note.len());
    let (pitch, octave) = note.split_at(split_at);
    let octave: i32 = if octave.is_empty() {
        4
    } else {
        octave.parse().map_err(|_| InvalidNote(note.clone()))?
    };

    let Some(letter) = pitch.get(..1) else {
        return Err(InvalidNote(note));
    };
    let accidental = &pitch[letter.len()..];
    let semitone = match letter {
        "c" => 0,
        "d" => 2,
        "e" => 4,
        "f" => 5,
        "g" => 7,
        "a" => 9,
        "b" => 11,
        _ => return Err(InvalidNote(note)),
    };
    let semitone = match accidental {
        "" => semitone,
        "#" => semitone + 1,
        "b" => semitone - 1,
        _ => return Err(InvalidNote(note)),
    };
    if !(-1..=9).contains(&octave) {
        return Err(InvalidNote(note));
    }
    let midi_note = 12 * (octave + 1) + semitone;
    if !(0..=127).contains(&midi_note) {
        return Err(InvalidNote(note));
    }
    let frequency = 440.0 * 2.0_f32.powf((midi_note - 69) as f32 / 12.0);

    Ok(frequency)
}

fn play_tone(frequency: f32) -> Result<(), Box<dyn Error>> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or("no default audio output device is available")?;
    let config = device.default_output_config()?;
    let sample_rate = config.sample_rate().0 as usize;
    let channels = config.channels() as usize;
    let tone = ToneParameters {
        channels,
        sample_rate,
        total_frames: sample_rate * DURATION_SECONDS,
        frequency,
    };
    let frames_played = Arc::new(AtomicUsize::new(0));
    let callback_failed = Arc::new(AtomicBool::new(false));

    let stream = build_stream_for_format(
        &device,
        config,
        tone,
        Arc::clone(&frames_played),
        Arc::clone(&callback_failed),
    )?;

    stream.play()?;
    while frames_played.load(Ordering::Acquire) < tone.total_frames
        && !callback_failed.load(Ordering::Acquire)
    {
        thread::sleep(Duration::from_millis(5));
    }

    if callback_failed.load(Ordering::Acquire) {
        return Err("audio playback failed".into());
    }
    thread::sleep(Duration::from_millis(100));
    Ok(())
}

fn build_stream_for_format(
    device: &cpal::Device,
    config: cpal::SupportedStreamConfig,
    tone: ToneParameters,
    frames_played: Arc<AtomicUsize>,
    callback_failed: Arc<AtomicBool>,
) -> Result<cpal::Stream, Box<dyn Error>> {
    let error_frames = Arc::clone(&frames_played);
    let error_failure = Arc::clone(&callback_failed);
    let error_handler = move |error| {
        eprintln!("Audio stream error: {error}");
        error_frames.store(tone.total_frames, Ordering::Release);
        error_failure.store(true, Ordering::Release);
    };

    match config.sample_format() {
        SampleFormat::F32 => {
            build_stream::<f32>(device, config, tone, frames_played, error_handler)
        }
        SampleFormat::I16 => {
            build_stream::<i16>(device, config, tone, frames_played, error_handler)
        }
        SampleFormat::U16 => {
            build_stream::<u16>(device, config, tone, frames_played, error_handler)
        }
        format => Err(format!("unsupported audio sample format: {format:?}").into()),
    }
}

fn build_stream<T>(
    device: &cpal::Device,
    config: cpal::SupportedStreamConfig,
    tone: ToneParameters,
    frames_played: Arc<AtomicUsize>,
    error_handler: impl FnMut(cpal::StreamError) + Send + 'static,
) -> Result<cpal::Stream, Box<dyn Error>>
where
    T: Sample + SizedSample + FromSample<f32>,
{
    let frames_for_callback = Arc::clone(&frames_played);
    let stream = device.build_output_stream(
        &config.into(),
        move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
            write_samples(data, tone, &frames_for_callback);
        },
        error_handler,
        None,
    )?;
    Ok(stream)
}

fn write_samples<T>(output: &mut [T], tone: ToneParameters, frames_played: &AtomicUsize)
where
    T: Sample + FromSample<f32>,
{
    let start_frame = frames_played.load(Ordering::Relaxed);
    for (frame_index, frame) in output.chunks_mut(tone.channels).enumerate() {
        let current_frame = start_frame + frame_index;
        let value = if current_frame < tone.total_frames {
            let phase = std::f32::consts::TAU * tone.frequency * current_frame as f32
                / tone.sample_rate as f32;
            phase.sin() * 0.25
        } else {
            0.0
        };

        for sample in frame {
            *sample = T::from_sample(value);
        }
    }
    frames_played.store(
        start_frame + output.len() / tone.channels,
        Ordering::Release,
    );
}

fn run() -> Result<(), Box<dyn Error>> {
    let mut arguments = env::args().skip(1);
    let Some(note) = arguments.next() else {
        return Err(USAGE.into());
    };
    if arguments.next().is_some() {
        return Err(USAGE.into());
    }

    let frequency = note_frequency(&note)?;
    println!("Playing {note} ({frequency:.2} Hz) for {DURATION_SECONDS} seconds");
    play_tone(frequency)
}

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::note_frequency;

    #[test]
    fn uses_a440_tuning_and_defaults_to_octave_four() {
        assert!((note_frequency("a").unwrap() - 440.0).abs() < 0.001);
        assert!((note_frequency("c").unwrap() - 261.6256).abs() < 0.01);
    }

    #[test]
    fn parses_sharps_flats_and_explicit_octaves() {
        assert!((note_frequency("f#").unwrap() - 369.9944).abs() < 0.01);
        assert!((note_frequency("bb3").unwrap() - 233.0819).abs() < 0.01);
        assert!((note_frequency("a3").unwrap() - 220.0).abs() < 0.001);
    }

    #[test]
    fn rejects_invalid_notes_and_out_of_range_octaves() {
        for note in ["h", "c##", "c10", "c-2", "", "♯4", "c♯4"] {
            assert!(note_frequency(note).is_err(), "accepted {note:?}");
        }
    }
}
