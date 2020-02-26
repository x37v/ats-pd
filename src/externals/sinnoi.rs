use crate::data::AtsData;
use itertools::izip;
use pd_ext::builder::SignalProcessorExternalBuilder;
use pd_ext::external::SignalProcessorExternal;
use pd_ext::post::PdPost;
use pd_ext::symbol::Symbol;
use rand::prelude::*;
use std::convert::TryInto;
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::sync::Arc;

const DSP_RECV_MAX: usize = 32;

enum Command {
    Data(Option<Arc<AtsData>>),
}

fn noise() -> f64 {
    thread_rng().gen_range(-1f64, 1f64)
}

lazy_static::lazy_static! {
    static ref ALL: Symbol = "all".try_into().unwrap();
    static ref FREQ_MUL: Symbol = "freq_mul".try_into().unwrap();
    static ref FREQ_ADD: Symbol = "freq_add".try_into().unwrap();
    static ref AMP_MUL: Symbol = "amp_mul".try_into().unwrap();
    static ref NOISE_MUL: Symbol = "noise_mul".try_into().unwrap();
    static ref NOISE_BW: Symbol = "noise_bw".try_into().unwrap();
}

pub struct ParitalSynth {
    phase_freq_mul: f64,
    phase: f64,
    noise_phase: f64,
    noise_x0: f64,
    noise_x1: f64,
    freq_mul: f64,
    freq_add: f64,
    amp_mul: f64,
    noise_amp_mul: f64,
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
            freq_mul: 1f64,
            freq_add: 0f64,
            amp_mul: 1f64,
            noise_amp_mul: 1f64,
            noise_bw_scale: 0.1f64,
        }
    }
}

impl ParitalSynth {
    pub fn synth(&mut self, freq: f64, sin_amp: f64, noise_energy: f64) -> f32 {
        //apply transformations
        //should freq scaling affect noise bandwidth and offset?
        let freq = freq * self.freq_mul + self.freq_add;
        let sin_amp = self.amp_mul * sin_amp;
        let noise_energy = noise_energy * self.noise_amp_mul;

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

        (sin * sin_amp + noise * sin * noise_energy) as f32
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
        synths: Box<[ParitalSynth]>,
        post: Box<dyn PdPost>,
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

        #[sel]
        pub fn freq_mul(&mut self, args: &[pd_ext::atom::Atom]) {
            match self.extract_args(args) {
                Ok((i, v)) => (),
                Err(e) => self.post.post_error(e),
            };
        }

        fn extract_args(&self, list: &[pd_ext::atom::Atom]) -> Result<(Option<usize>, f64), String> {
            if list.len() != 2 {
                return Err("expected 2 arguments".into());
            }
            let mut index = None;
            if let Some(i) = list[0].get_int() {
                index = Some(i as usize);
            } else {
                let s = list[0].get_symbol();
                if s.is_none() || s.unwrap() != *ALL {
                    return Err("expect first arg to be an index or 'all'".into());
                }
            }
            let val = list[1].get_float();
            if val.is_none() {
                return Err("expect second arg to be a float".into());
            }
            let val = val.unwrap() as f64;
            Ok((index, val))
        }

    }

    impl SignalProcessorExternal for AtsSinNoiExternal {
        fn new(builder: &mut dyn SignalProcessorExternalBuilder<Self>) -> Self {
            builder.new_signal_outlet();
            let (data_send, data_recv) = sync_channel(32);

            let synths = 50; //TODO get from args
            let synths = (0..synths).map(|_| ParitalSynth::default()).collect();

            Self {
                current: None,
                data_send,
                data_recv,
                synths,
                post: builder.poster()
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
                let frames = c.frames.len() as isize;
                for (out, pos) in outputs[0].iter_mut().zip(inputs[0].iter()) {
                    let pos = (*pos as f64) * pmul;
                    let mut p0 = pos.floor() as isize;
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
                        *out = *out + s.synth(f, a, n);
                    }
                }
            }
        }
    }
}

fn lerp(x0: f64, x1: f64, frac: f64) -> f64 {
    x0 + (x1 - x0) * frac
}
