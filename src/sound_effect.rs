use anyhow::{Context as _, Result};
use rodio::{ChannelCount, DeviceSinkBuilder, MixerDeviceSink, Player, SampleRate, Source};
use std::f32::consts::TAU;
use std::num::{NonZeroU16, NonZeroU32};
use std::time::Duration;

const SAMPLE_RATE: u32 = 48_000;
const TOTAL_DURATION: Duration = Duration::from_millis(11_400);
const TOTAL_SAMPLES: usize = 547_200;
const BEEP_TIMES: [f32; 14] = [
    0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 8.5, 9.0, 9.25, 9.5, 9.75,
];

#[derive(Default)]
pub(crate) struct SoundEffectController {
    output_stream: Option<MixerDeviceSink>,
    player: Option<Player>,
}

impl SoundEffectController {
    pub(crate) fn play_c4_sequence(&mut self) -> Result<()> {
        self.stop();
        if self.output_stream.is_none() {
            self.output_stream = Some(
                DeviceSinkBuilder::open_default_sink()
                    .context("no default audio output device available")?,
            );
        }
        let output_stream = self
            .output_stream
            .as_ref()
            .context("audio output stream was not initialized")?;
        let player = Player::connect_new(output_stream.mixer());
        player.set_volume(0.72);
        player.append(C4Sequence::new());
        self.player = Some(player);
        Ok(())
    }

    pub(crate) fn stop(&mut self) {
        if let Some(player) = self.player.take() {
            player.stop();
        }
        self.output_stream = None;
    }
}

#[derive(Clone)]
struct C4Sequence {
    sample_index: usize,
    noise_state: u32,
}

impl C4Sequence {
    fn new() -> Self {
        Self {
            sample_index: 0,
            noise_state: 0x7355_6081,
        }
    }

    fn beep_sample(time: f32) -> f32 {
        BEEP_TIMES.iter().fold(0.0, |sample, beep_time| {
            let local = time - beep_time;
            if (0.0..0.075).contains(&local) {
                let envelope = 1.0 - local / 0.075;
                sample + (TAU * 1_180.0 * local).sin() * envelope * 0.34
            } else {
                sample
            }
        })
    }

    #[allow(clippy::cast_precision_loss)]
    fn explosion_sample(&mut self, time: f32) -> f32 {
        let local = time - 10.0;
        if !(0.0..1.4).contains(&local) {
            return 0.0;
        }
        self.noise_state ^= self.noise_state << 13;
        self.noise_state ^= self.noise_state >> 17;
        self.noise_state ^= self.noise_state << 5;
        let noise = (self.noise_state as f32 / u32::MAX as f32) * 2.0 - 1.0;
        let envelope = (1.0 - local / 1.4).powi(3);
        let frequency = 92.0 - 55.0 * (local / 1.4);
        let low = (TAU * frequency * local).sin();
        (low * 0.58 + noise * 0.42) * envelope
    }
}

impl Iterator for C4Sequence {
    type Item = f32;

    #[allow(clippy::cast_precision_loss)]
    fn next(&mut self) -> Option<Self::Item> {
        if self.sample_index >= TOTAL_SAMPLES {
            return None;
        }
        let time = self.sample_index as f32 / SAMPLE_RATE as f32;
        self.sample_index += 1;
        Some((Self::beep_sample(time) + self.explosion_sample(time)).clamp(-1.0, 1.0))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = TOTAL_SAMPLES.saturating_sub(self.sample_index);
        (remaining, Some(remaining))
    }
}

impl ExactSizeIterator for C4Sequence {}

impl Source for C4Sequence {
    fn current_span_len(&self) -> Option<usize> {
        Some(self.len())
    }

    fn channels(&self) -> ChannelCount {
        NonZeroU16::new(1).unwrap_or(NonZeroU16::MIN)
    }

    fn sample_rate(&self) -> SampleRate {
        NonZeroU32::new(SAMPLE_RATE).unwrap_or(NonZeroU32::MIN)
    }

    fn total_duration(&self) -> Option<Duration> {
        Some(TOTAL_DURATION)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synthesized_sequence_is_finite_and_bounded() {
        let source = C4Sequence::new();
        assert_eq!(source.total_duration(), Some(TOTAL_DURATION));
        assert_eq!(source.len(), TOTAL_SAMPLES);
        assert!(
            source
                .into_iter()
                .all(|sample| sample.is_finite() && sample.abs() <= 1.0)
        );
    }
}
