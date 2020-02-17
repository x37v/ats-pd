use crate::data::AtsData;
use itertools::izip;
use pd_ext::builder::SignalProcessorExternalBuilder;
use pd_ext::external::SignalProcessorExternal;
use rand::prelude::*;
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::sync::Arc;

const DSP_RECV_MAX: usize = 32;

enum Command {
    Data(Option<Arc<AtsData>>),
}

fn noise() -> f64 {
    thread_rng().gen_range(-1f64, 1f64)
}

pub struct ParitalSynth {
    phase_freq_mul: f64,
    phase: f64,
    noise_phase: f64,
    noise_x0: f64,
    noise_x1: f64,
    noise_bw_scale: f64,
}

impl Default for ParitalSynth {
    fn default() -> Self {
        Self {
            phase_freq_mul: 1f64 / 44100f64,
            phase: 0.into(),
            noise_phase: 0.into(),
            noise_x0: noise(),
            noise_x1: noise(),
            noise_bw_scale: 0.1f64,
        }
    }
}

impl ParitalSynth {
    pub fn synth(&mut self, freq: f64, noise_energy: f64) -> f32 {
        //TODO if freq > 500 { 1 } else { 0.25 } * bw...
        let noise_bw = freq * self.noise_bw_scale;

        self.phase = (self.phase + freq * self.phase_freq_mul).fract();
        self.noise_phase = self.noise_phase + noise_bw * self.phase_freq_mul;
        if self.noise_phase >= 1f64 {
            self.noise_phase = self.noise_phase.fract();
            self.noise_x0 = self.noise_x1;
            self.noise_x1 = noise();
        }

        let sin = (2f64 * std::f64::consts::PI * self.phase).sin();
        let noise = lerp(self.noise_x0, self.noise_x1, self.noise_phase);

        (sin + noise * sin * noise_energy) as f32
    }

    pub fn sample_rate(&mut self, sr: f64) {
        self.phase_freq_mul = 1f64 / sr;
    }
}

pd_ext_macros::external! {
    #[name = "ats/sinnoi~"]
    pub struct AtsSinNoiExternal {
        current: Option<Arc<AtsData>>,
        data_send: SyncSender<Command>,
        data_recv: Receiver<Command>,
        synths: Box<[ParitalSynth]>
    }

    impl AtsSinNoiExternal {
        #[sel]
        pub fn ats_data(&mut self, key: pd_ext::symbol::Symbol) {
            let d = crate::cache::get(key);
            let _ = self.data_send.try_send(Command::Data(d));
            //TODO warn if empty?
        }

        #[sel]
        pub fn clear(&mut self) {
            let _ = self.data_send.send(Command::Data(None));
        }
    }

    impl SignalProcessorExternal for AtsSinNoiExternal {
        fn new(builder: &mut dyn SignalProcessorExternalBuilder<Self>) -> Self {
            builder.new_signal_outlet();
            let (data_send, data_recv) = sync_channel(32);

            let synths = 50;
            let synths = (0..synths).map(|_| ParitalSynth::default()).collect();

            Self {
                current: None,
                data_send,
                data_recv,
                synths
            }
        }

        fn process(
            &mut self,
            _frames: usize,
            inputs: &[&mut [pd_sys::t_float]],
            outputs: &mut [&mut [pd_sys::t_float]],
        ) {
            let mut cnt = 0;
            while let Ok(c) = self.data_recv.try_recv() {
                match c {
                    Command::Data(c) => self.current = c
                }
                cnt = cnt + 1;
                if cnt > DSP_RECV_MAX {
                    break;
                }
            }

            if let Some(c) = &self.current {
                let with_noise = c.has_noise();
                let pmul = c.header.fra / c.header.dur;
                //TODO handle offset
                let range = 0..std::cmp::min(c.partials(), self.synths.len());
                let synths = &mut self.synths[range.clone()];
                for (out, pos) in outputs[0].iter_mut().zip(inputs[0].iter()) {
                    let pos = (*pos as f64) * pmul;
                    let mut p0 = pos.floor() as isize;
                    let frames = c.frames.len() as isize;
                    let mut fract = 0f64;
                    let mut in_range = false;
                    if p0 < 0 {
                        p0 = 0;
                    } else if p0 + 1 >= frames {
                        p0 = frames - 2;
                        fract = 1f64;
                    } else {
                        fract = pos.fract();
                        in_range = true;
                    }
                    let p0 = p0 as usize;

                    let f0 = &c.frames[p0];
                    let f1 = &c.frames[p0 + 1];
                    for (s, p0, p1) in izip!(synths.iter_mut(), f0[range.clone()].iter(), f1[range.clone()].iter()) {
                        let f = lerp(p0.freq, p1.freq, fract);
                        let (a, n) = if in_range {
                            (
                            lerp(p0.amp, p1.amp, fract),
                            if with_noise {
                                lerp(p0.noise_energy.unwrap(), p1.noise_energy.unwrap(), fract)
                            } else {
                                0f64
                            })
                        } else {
                            (0f64, 0f64)
                        };
                        *out = *out + s.synth(f, n) * (a as f32);
                    }
                }
            }
        }
    }
}

fn lerp(x0: f64, x1: f64, frac: f64) -> f64 {
    x0 + (x1 - x0) * frac
}
