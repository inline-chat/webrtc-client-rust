use webrtc_audio_processing::Stats;

use super::processor::AudioEchoProcessor;

pub struct EncoderProcess {
    processor: AudioEchoProcessor,
    channels: u16,
    sample_rate: u32,
}

impl EncoderProcess {
    pub fn new(processor: AudioEchoProcessor, channels: u16, sample_rate: u32) -> Self {
        Self {
            processor,
            sample_rate,
            channels,
        }
    }

    pub fn frame_size(&self) -> usize {
        self.processor.num_samples_per_frame() * self.channels as usize
    }

    pub fn set_output_will_be_muted(&self, muted: bool) {
        self.processor.set_output_will_be_muted(muted);
    }

    pub fn stats(&self) -> Stats {
        self.processor.get_stats()
    }

    pub fn get_echo_processor(&self) -> &AudioEchoProcessor {
        &self.processor
    }
    pub fn get_echo_processor_mut(&mut self) -> &mut AudioEchoProcessor {
        &mut self.processor
    }

    pub fn preallocate_buffer(&self) -> Vec<f32> {
        let one_ms = (self.sample_rate as usize / 1000) * self.channels as usize;
        // allocate 80ms buffer (random number)
        vec![0.0_f32; one_ms * 100]
    }
}
