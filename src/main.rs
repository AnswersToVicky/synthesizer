use std::{
    env,
    error::Error,
    fmt,
    sync::{
        atomic::{AtomicBool, AtomicU32, AtomicU8, AtomicUsize, Ordering},
        Arc,
    },
    thread,
    time::Duration,
};

use cpal::{
    traits::{DeviceTrait, HostTrait, StreamTrait},
    FromSample, Sample, SampleFormat, SizedSample,
};
use eframe::egui;
use midir::{Ignore, MidiInput};

const DURATION_SECONDS: usize = 2;
const MIDI_ATTACK_SECONDS: f32 = 0.005;
const MIDI_RELEASE_SECONDS: f32 = 0.03;
const USAGE: &str =
    "Usage: synthesizer <note> | synthesizer midi\nExamples: synthesizer c, synthesizer 'f#', synthesizer midi";

#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum Waveform {
    Sine,
    Sawtooth,
    Pulse,
}

#[derive(Clone, Copy)]
struct ToneParameters {
    channels: usize,
    sample_rate: usize,
    total_frames: usize,
    frequency: f32,
}

struct MidiVoices {
    velocities: [AtomicU8; 128],
}

impl MidiVoices {
    fn new() -> Self {
        Self {
            velocities: std::array::from_fn(|_| AtomicU8::new(0)),
        }
    }
}

struct MidiGui {
    waveform: Waveform,
    selected_waveform: Arc<AtomicU8>,
    volume_percent: f32,
    volume: Arc<AtomicU32>,
    callback_failed: Arc<AtomicBool>,
    should_exit: Arc<AtomicBool>,
}

impl MidiGui {
    fn new(
        selected_waveform: Arc<AtomicU8>,
        volume: Arc<AtomicU32>,
        callback_failed: Arc<AtomicBool>,
        should_exit: Arc<AtomicBool>,
    ) -> Self {
        Self {
            waveform: Waveform::Sine,
            selected_waveform,
            volume_percent: 100.0,
            volume,
            callback_failed,
            should_exit,
        }
    }

    fn select_waveform(&mut self, waveform: Waveform) {
        self.waveform = waveform;
        self.selected_waveform
            .store(waveform as u8, Ordering::Release);
    }
}

impl eframe::App for MidiGui {
    fn update(&mut self, context: &egui::Context, _frame: &mut eframe::Frame) {
        if self.callback_failed.load(Ordering::Acquire) || self.should_exit.load(Ordering::Acquire)
        {
            context.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }

        egui::CentralPanel::default().show(context, |ui| {
            ui.heading("MIDI Oscillator");
            ui.horizontal(|ui| {
                ui.allocate_ui_with_layout(
                    egui::vec2(48.0, 150.0),
                    egui::Layout::top_down(egui::Align::Center),
                    |ui| {
                        ui.label("Volume");
                        if ui
                            .add_sized(
                                [24.0, 100.0],
                                egui::Slider::new(&mut self.volume_percent, 0.0..=100.0)
                                    .vertical()
                                    .show_value(false),
                            )
                            .changed()
                        {
                            self.volume
                                .store((self.volume_percent / 100.0).to_bits(), Ordering::Release);
                        }
                        ui.label(format!("{:.0}%", self.volume_percent));
                    },
                );
                ui.separator();
                ui.allocate_ui_with_layout(
                    egui::vec2(320.0, 150.0),
                    egui::Layout::top_down(egui::Align::Center),
                    |ui| {
                        ui.label("Choose a waveform:");
                        ui.horizontal(|ui| {
                            if ui
                                .add_sized(
                                    [100.0, 36.0],
                                    egui::Button::new("Sine")
                                        .selected(self.waveform == Waveform::Sine),
                                )
                                .clicked()
                            {
                                self.select_waveform(Waveform::Sine);
                            }
                            if ui
                                .add_sized(
                                    [100.0, 36.0],
                                    egui::Button::new("Sawtooth")
                                        .selected(self.waveform == Waveform::Sawtooth),
                                )
                                .clicked()
                            {
                                self.select_waveform(Waveform::Sawtooth);
                            }
                            if ui
                                .add_sized(
                                    [100.0, 36.0],
                                    egui::Button::new("Pulse")
                                        .selected(self.waveform == Waveform::Pulse),
                                )
                                .clicked()
                            {
                                self.select_waveform(Waveform::Pulse);
                            }
                        });
                    },
                );
            });
        });
    }
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

fn play_midi() -> Result<(), Box<dyn Error>> {
    let mut midi_input = MidiInput::new("Synthesizer MIDI input")?;
    midi_input.ignore(Ignore::None);
    let ports = midi_input.ports();
    if ports.is_empty() {
        return Err("no MIDI input devices found".into());
    }
    let port_names = ports
        .iter()
        .map(|port| midi_input.port_name(port))
        .collect::<Result<Vec<_>, _>>()?;
    let selected_index = select_midi_port(&port_names)?;
    let selected_port = &ports[selected_index];
    let port_name = midi_input.port_name(selected_port)?;
    let voices = Arc::new(MidiVoices::new());
    let voices_for_input = Arc::clone(&voices);
    let connection = midi_input.connect(
        selected_port,
        "synthesizer MIDI",
        move |_timestamp, message, _| {
            handle_midi_message(&voices_for_input, message);
        },
        (),
    )?;

    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or("no default audio output device is available")?;
    let config = device.default_output_config()?;
    let sample_rate = config.sample_rate().0 as usize;
    let channels = config.channels() as usize;
    let callback_failed = Arc::new(AtomicBool::new(false));
    let selected_waveform = Arc::new(AtomicU8::new(Waveform::Sine as u8));
    let volume = Arc::new(AtomicU32::new(1.0_f32.to_bits()));
    let stream = build_midi_stream(
        &device,
        config,
        channels,
        sample_rate,
        Arc::clone(&voices),
        Arc::clone(&selected_waveform),
        Arc::clone(&volume),
        Arc::clone(&callback_failed),
    )?;

    stream.play()?;
    println!("MIDI input: {port_name}. Play notes; close the window to exit.");
    let should_exit = Arc::new(AtomicBool::new(false));
    let exit_flag = Arc::clone(&should_exit);
    ctrlc::set_handler(move || exit_flag.store(true, Ordering::Release))?;
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([440.0, 180.0])
            .with_min_inner_size([440.0, 180.0]),
        ..Default::default()
    };
    let callback_failed_for_gui = Arc::clone(&callback_failed);
    let should_exit_for_gui = Arc::clone(&should_exit);
    let volume_for_gui = Arc::clone(&volume);
    eframe::run_native(
        "MIDI Synthesizer",
        options,
        Box::new(move |_creation_context| {
            Ok(Box::new(MidiGui::new(
                selected_waveform,
                volume_for_gui,
                callback_failed_for_gui,
                should_exit_for_gui,
            )))
        }),
    )?;

    if callback_failed.load(Ordering::Acquire) {
        return Err("audio playback failed".into());
    }

    drop(connection);
    drop(stream);
    Ok(())
}

fn build_midi_stream(
    device: &cpal::Device,
    config: cpal::SupportedStreamConfig,
    channels: usize,
    sample_rate: usize,
    voices: Arc<MidiVoices>,
    selected_waveform: Arc<AtomicU8>,
    volume: Arc<AtomicU32>,
    callback_failed: Arc<AtomicBool>,
) -> Result<cpal::Stream, Box<dyn Error>> {
    let failure_for_callback = Arc::clone(&callback_failed);
    let error_handler = move |error| {
        eprintln!("Audio stream error: {error}");
        failure_for_callback.store(true, Ordering::Release);
    };
    match config.sample_format() {
        SampleFormat::F32 => build_midi_stream_for_sample::<f32>(
            device,
            config,
            channels,
            sample_rate,
            voices,
            selected_waveform,
            volume,
            error_handler,
        ),
        SampleFormat::I16 => build_midi_stream_for_sample::<i16>(
            device,
            config,
            channels,
            sample_rate,
            voices,
            selected_waveform,
            volume,
            error_handler,
        ),
        SampleFormat::U16 => build_midi_stream_for_sample::<u16>(
            device,
            config,
            channels,
            sample_rate,
            voices,
            selected_waveform,
            volume,
            error_handler,
        ),
        format => Err(format!("unsupported audio sample format: {format:?}").into()),
    }
}

fn build_midi_stream_for_sample<T>(
    device: &cpal::Device,
    config: cpal::SupportedStreamConfig,
    channels: usize,
    sample_rate: usize,
    voices: Arc<MidiVoices>,
    selected_waveform: Arc<AtomicU8>,
    volume: Arc<AtomicU32>,
    error_handler: impl FnMut(cpal::StreamError) + Send + 'static,
) -> Result<cpal::Stream, Box<dyn Error>>
where
    T: Sample + SizedSample + FromSample<f32>,
{
    let mut phases = [0.0; 128];
    let mut envelopes = [0.0; 128];
    let stream = device.build_output_stream(
        &config.into(),
        move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
            write_midi_samples(
                data,
                channels,
                sample_rate,
                &voices,
                &selected_waveform,
                &volume,
                &mut phases,
                &mut envelopes,
            );
        },
        error_handler,
        None,
    )?;
    Ok(stream)
}

fn write_midi_samples<T>(
    output: &mut [T],
    channels: usize,
    sample_rate: usize,
    voices: &MidiVoices,
    selected_waveform: &AtomicU8,
    volume: &AtomicU32,
    phases: &mut [f32; 128],
    envelopes: &mut [f32; 128],
) where
    T: Sample + FromSample<f32>,
{
    let attack_step = 1.0 / (sample_rate as f32 * MIDI_ATTACK_SECONDS);
    let release_step = 1.0 / (sample_rate as f32 * MIDI_RELEASE_SECONDS);
    let volume = f32::from_bits(volume.load(Ordering::Acquire));
    for frame in output.chunks_mut(channels) {
        let mut mixed_sample = 0.0;
        let mut active_voices = 0;
        let waveform = match selected_waveform.load(Ordering::Acquire) {
            value if value == Waveform::Sawtooth as u8 => Waveform::Sawtooth,
            value if value == Waveform::Pulse as u8 => Waveform::Pulse,
            _ => Waveform::Sine,
        };
        for (note, (phase, envelope)) in phases.iter_mut().zip(envelopes.iter_mut()).enumerate() {
            let velocity = voices.velocities[note].load(Ordering::Acquire);
            let target = velocity as f32 / 127.0;
            let step = if target > *envelope {
                attack_step
            } else {
                release_step
            };
            *envelope = move_envelope_towards(*envelope, target, step);
            let frequency = midi_note_frequency(note as u8);
            mixed_sample += waveform_sample(*phase, waveform) * *envelope;
            *phase = (*phase + std::f32::consts::TAU * frequency / sample_rate as f32)
                % std::f32::consts::TAU;
            if *envelope > 0.0 {
                active_voices += 1;
            }
        }
        if active_voices > 0 {
            mixed_sample *= 0.2 * volume / (active_voices as f32).sqrt();
        }

        for sample in frame {
            *sample = T::from_sample(mixed_sample);
        }
    }
}

fn waveform_sample(phase: f32, waveform: Waveform) -> f32 {
    let cycle = phase / std::f32::consts::TAU;
    match waveform {
        Waveform::Sine => phase.sin(),
        Waveform::Sawtooth => 1.0 - 4.0 * (cycle - 0.5).abs(),
        Waveform::Pulse => {
            if cycle < 0.5 {
                1.0
            } else {
                -1.0
            }
        }
    }
}

fn move_envelope_towards(current: f32, target: f32, step: f32) -> f32 {
    if current < target {
        (current + step).min(target)
    } else {
        (current - step).max(target)
    }
}

fn handle_midi_message(voices: &MidiVoices, message: &[u8]) {
    if message.len() < 3 {
        return;
    }

    let status = message[0] & 0xf0;
    let note = message[1] as usize;
    if note >= voices.velocities.len() {
        return;
    }
    let velocity = message[2];
    match status {
        0x90 if velocity > 0 => voices.velocities[note].store(velocity, Ordering::Release),
        0x80 | 0x90 => voices.velocities[note].store(0, Ordering::Release),
        _ => {}
    }
}

fn midi_note_frequency(note: u8) -> f32 {
    440.0 * 2.0_f32.powf((note as f32 - 69.0) / 12.0)
}

fn select_midi_port(port_names: &[String]) -> Result<usize, String> {
    let matching_akai: Vec<_> = port_names
        .iter()
        .enumerate()
        .filter(|(_, name)| name.to_ascii_lowercase().contains("mpk"))
        .map(|(index, _)| index)
        .collect();
    let primary_midi_ports: Vec<_> = matching_akai
        .iter()
        .copied()
        .filter(|index| {
            port_names[*index]
                .to_ascii_lowercase()
                .contains("midi port")
        })
        .collect();

    if primary_midi_ports.len() == 1 {
        return Ok(primary_midi_ports[0]);
    }
    if matching_akai.len() == 1 {
        return Ok(matching_akai[0]);
    }
    if matching_akai.is_empty() && port_names.len() == 1 {
        return Ok(0);
    }

    Err(format!(
        "could not select a MIDI input automatically; available inputs: {}",
        port_names.join(", ")
    ))
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

    if note.eq_ignore_ascii_case("midi") {
        return play_midi();
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
    use super::{
        handle_midi_message, midi_note_frequency, move_envelope_towards, note_frequency,
        select_midi_port, waveform_sample, MidiVoices, Waveform,
    };

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

    #[test]
    fn midi_note_numbers_use_a440_tuning() {
        assert!((midi_note_frequency(69) - 440.0).abs() < 0.001);
        assert!((midi_note_frequency(60) - note_frequency("c").unwrap()).abs() < 0.001);
    }

    #[test]
    fn oscillator_waveforms_have_expected_shapes() {
        let quarter_cycle = std::f32::consts::FRAC_PI_2;
        assert!((waveform_sample(quarter_cycle, Waveform::Sine) - 1.0).abs() < 0.001);
        assert!((waveform_sample(0.0, Waveform::Sawtooth) + 1.0).abs() < 0.001);
        assert!((waveform_sample(std::f32::consts::PI, Waveform::Sawtooth) - 1.0).abs() < 0.001);
        assert_eq!(waveform_sample(0.0, Waveform::Pulse), 1.0);
        assert_eq!(waveform_sample(std::f32::consts::PI, Waveform::Pulse), -1.0);
    }

    #[test]
    fn midi_note_on_and_off_update_voice_velocity() {
        let voices = MidiVoices::new();
        handle_midi_message(&voices, &[0x92, 60, 100]);
        assert_eq!(
            voices.velocities[60].load(std::sync::atomic::Ordering::Acquire),
            100
        );

        handle_midi_message(&voices, &[0x82, 60, 64]);
        assert_eq!(
            voices.velocities[60].load(std::sync::atomic::Ordering::Acquire),
            0
        );

        handle_midi_message(&voices, &[0x90, 60, 100]);
        handle_midi_message(&voices, &[0x90, 60, 0]);
        assert_eq!(
            voices.velocities[60].load(std::sync::atomic::Ordering::Acquire),
            0
        );
    }

    #[test]
    fn midi_envelope_releases_gradually_and_reaches_silence() {
        let release_step = 0.25;
        let first_release_sample = move_envelope_towards(1.0, 0.0, release_step);
        assert_eq!(first_release_sample, 0.75);

        let mut envelope = first_release_sample;
        for _ in 0..3 {
            envelope = move_envelope_towards(envelope, 0.0, release_step);
        }
        assert_eq!(envelope, 0.0);
    }

    #[test]
    fn prefers_the_akai_musical_input_over_auxiliary_ports() {
        let ports = vec![
            "MPK mini IV MIDI Port".to_string(),
            "MPK mini IV DAW Port".to_string(),
            "MPK mini IV Plugin Port".to_string(),
            "MPK mini IV Software Control Port".to_string(),
        ];
        assert_eq!(select_midi_port(&ports), Ok(0));
    }
}
