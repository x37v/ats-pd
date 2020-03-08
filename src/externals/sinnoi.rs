use crate::data::AtsData;
use atomic::Atomic;
use itertools::izip;
use pd_ext::builder::SignalProcessorExternalBuilder;
use pd_ext::external::{SignalProcessor, SignalProcessorExternal};
use pd_ext::post::PdPost;
use pd_ext::symbol::Symbol;
use rand::prelude::*;
use std::convert::TryInto;
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::sync::Arc;

const DSP_RECV_MAX: usize = 32;
const STORE_ORDERING: std::sync::atomic::Ordering = std::sync::atomic::Ordering::Relaxed;
const LOAD_ORDERING: std::sync::atomic::Ordering = std::sync::atomic::Ordering::Relaxed;

type ArcAtomic<T> = Arc<Atomic<T>>;

enum Command {
    Data(Option<Arc<AtsData>>),
}

fn noise() -> f64 {
    thread_rng().gen_range(-1f64, 1f64)
}

lazy_static::lazy_static! {
    static ref ALL: Symbol = "all".try_into().unwrap();
}

pub struct ParitalSynth {
    phase_freq_mul: f64,
    phase: f64,
    noise_phase: f64,
    noise_x0: f64,
    noise_x1: f64,

    //params
    freq_mul: ArcAtomic<f64>,
    freq_add: ArcAtomic<f64>,
    amp_mul: ArcAtomic<f64>,
    noise_amp_mul: ArcAtomic<f64>,
    noise_bw_scale: ArcAtomic<f64>,
}

struct ParitalSynthHandle {
    freq_mul: ArcAtomic<f64>,
    freq_add: ArcAtomic<f64>,
    amp_mul: ArcAtomic<f64>,
    noise_amp_mul: ArcAtomic<f64>,
    noise_bw_scale: ArcAtomic<f64>,
}

impl ParitalSynthHandle {
    pub fn freq_mul(&mut self, v: f64) {
        self.freq_mul.store(v, STORE_ORDERING);
    }

    pub fn freq_add(&mut self, v: f64) {
        self.freq_add.store(v, STORE_ORDERING);
    }

    pub fn amp_mul(&mut self, v: f64) {
        self.amp_mul.store(v, STORE_ORDERING);
    }

    pub fn noise_amp_mul(&mut self, v: f64) {
        self.noise_amp_mul.store(v, STORE_ORDERING);
    }

    pub fn noise_bw_scale(&mut self, v: f64) {
        self.noise_bw_scale.store(v, STORE_ORDERING);
    }

    pub fn new() -> (Self, ParitalSynth) {
        let freq_mul = Arc::new(Atomic::new(1f64));
        let freq_add = Arc::new(Atomic::new(0f64));
        let amp_mul = Arc::new(Atomic::new(1f64));
        let noise_amp_mul = Arc::new(Atomic::new(1f64));
        let noise_bw_scale = Arc::new(Atomic::new(0.1f64));
        (
            Self {
                freq_mul: freq_mul.clone(),
                freq_add: freq_add.clone(),
                amp_mul: amp_mul.clone(),
                noise_amp_mul: noise_amp_mul.clone(),
                noise_bw_scale: noise_bw_scale.clone(),
            },
            ParitalSynth::new(freq_mul, freq_add, amp_mul, noise_amp_mul, noise_bw_scale),
        )
    }
}

impl ParitalSynth {
    fn new(
        freq_mul: ArcAtomic<f64>,
        freq_add: ArcAtomic<f64>,
        amp_mul: ArcAtomic<f64>,
        noise_amp_mul: ArcAtomic<f64>,
        noise_bw_scale: ArcAtomic<f64>,
    ) -> Self {
        Self {
            phase_freq_mul: 1f64 / 44100f64,
            phase: 0.into(),
            noise_phase: 0.into(),
            noise_x0: noise(),
            noise_x1: noise(),

            freq_mul,
            freq_add,
            amp_mul,
            noise_amp_mul,
            noise_bw_scale,
        }
    }
}

impl ParitalSynth {
    pub fn synth(&mut self, freq: f64, sin_amp: f64, noise_energy: f64) -> f32 {
        //TODO interpolate
        let freq_mul = self.freq_mul.load(LOAD_ORDERING);
        let freq_add = self.freq_add.load(LOAD_ORDERING);
        let amp_mul = self.amp_mul.load(LOAD_ORDERING);
        let noise_amp_mul = self.noise_amp_mul.load(LOAD_ORDERING);
        let noise_bw_scale = self.noise_bw_scale.load(LOAD_ORDERING);

        //apply transformations
        //should freq scaling affect noise bandwidth and offset?
        let freq = freq * freq_mul + freq_add;
        let sin_amp = amp_mul * sin_amp;
        let noise_energy = noise_energy * noise_amp_mul;

        //TODO if freq > 500 { 1 } else { 0.25 } * bw...
        let noise_bw = freq * noise_bw_scale;

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

    /*
    pub fn sample_rate(&mut self, sr: f64) {
        self.phase_freq_mul = 1f64 / sr;
    }
    */
}

pub struct AtsSinNoiProcessor {
    current: Option<Arc<AtsData>>,
    data_recv: Receiver<Command>,
    synths: Box<[ParitalSynth]>,
}

impl SignalProcessor for AtsSinNoiProcessor {
    fn process(
        &mut self,
        _frames: usize,
        inputs: &[&mut [pd_sys::t_float]],
        outputs: &mut [&mut [pd_sys::t_float]],
    ) {
        let mut cnt = 0;
        while let Ok(c) = self.data_recv.try_recv() {
            match c {
                Command::Data(c) => self.current = c,
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
                *out = 0 as pd_sys::t_float;
                for (s, p0, p1) in izip!(
                    synths.iter_mut(),
                    f0[range.clone()].iter(),
                    f1[range.clone()].iter()
                ) {
                    let f = lerp(p0.freq, p1.freq, fract);
                    let (a, n) = if in_range {
                        (
                            lerp(p0.amp, p1.amp, fract),
                            if with_noise {
                                lerp(p0.noise_energy.unwrap(), p1.noise_energy.unwrap(), fract)
                            } else {
                                0f64
                            },
                        )
                    } else {
                        (0f64, 0f64)
                    };
                    *out = *out + s.synth(f, a, n);
                }
            }
        }
    }
}

pd_ext_macros::external! {
    #[name = "ats/sinnoi~"]
    pub struct AtsSinNoiExternal {
        data_send: SyncSender<Command>,
        handles: Box<[ParitalSynthHandle]>,
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
            self.apply_if(args, |s, v| s.freq_mul(v));
        }

        #[sel]
        pub fn freq_add(&mut self, args: &[pd_ext::atom::Atom]) {
            self.apply_if(args, |s, v| s.freq_add(v));
        }

        #[sel]
        pub fn amp_mul(&mut self, args: &[pd_ext::atom::Atom]) {
            self.apply_if(args, |s, v| s.amp_mul(v));
        }

        #[sel]
        pub fn noise_amp_mul(&mut self, args: &[pd_ext::atom::Atom]) {
            self.apply_if(args, |s, v| s.noise_amp_mul(v));
        }

        #[sel]
        pub fn noise_bw_scale(&mut self, args: &[pd_ext::atom::Atom]) {
            self.apply_if(args, |s, v| s.noise_bw_scale(v));
        }

        fn apply_if<F: Fn(&mut ParitalSynthHandle, f64)>(&mut self, args: &[pd_ext::atom::Atom], f: F) {
            match self.extract_args(args) {
                Ok((i, v)) =>
                    if let Some(i) = i {
                        if i < self.handles.len() {
                            f(&mut self.handles[i], v)
                        }
                    } else {
                        for s in self.handles.iter_mut() {
                            f(s, v);
                        }
                    },
                Err(msg) => self.post.post_error(msg)
            }
        }

        fn extract_args(&self, list: &[pd_ext::atom::Atom]) -> Result<(Option<usize>, f64), String> {
            if list.len() != 2 {
                return Err("expected 2 arguments".into());
            }
            let mut index = None;
            if let Some(i) = list[0].get_int() {
                let i = i as usize;
                if i > self.handles.len() {
                    return Err(format!("partial index {} out of range", i));
                }
                index = Some(i);
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
        fn new(builder: &mut dyn SignalProcessorExternalBuilder<Self>) -> Result<(Self, Box<dyn SignalProcessor>), String> {
            builder.new_signal_outlet();
            let (data_send, data_recv) = sync_channel(32);

            let mut synths = Vec::new();
            let mut handles = Vec::new();
            for _ in 0..50 { //TODO get count from args
                let (h, s) = ParitalSynthHandle::new();
                handles.push(h);
                synths.push(s);
            }

            Ok((
            Self {
                data_send,
                handles: handles.into(),
                post: builder.poster()
            },
            Box::new(AtsSinNoiProcessor {
                current: None,
                data_recv,
                synths: synths.into(),
            })))
        }
    }
}

fn lerp(x0: f64, x1: f64, frac: f64) -> f64 {
    x0 + (x1 - x0) * frac
}
