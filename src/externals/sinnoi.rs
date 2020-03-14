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

    cur_freq_mul: f64,
    cur_freq_add: f64,
    cur_amp_mul: f64,
    cur_noise_amp_mul: f64,
    cur_noise_bw_scale: f64,

    //params
    freq_mul: ArcAtomic<f64>,
    freq_add: ArcAtomic<f64>,
    amp_mul: ArcAtomic<f64>,
    noise_amp_mul: ArcAtomic<f64>,
    noise_bw_scale: ArcAtomic<f64>,

    inc_freq_mul: ArcAtomic<f64>,
    inc_freq_add: ArcAtomic<f64>,
    inc_amp_mul: ArcAtomic<f64>,
    inc_noise_amp_mul: ArcAtomic<f64>,
    inc_noise_bw_scale: ArcAtomic<f64>,
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
            phase_freq_mul: 1f64 / pd_ext::pd::sample_rate() as f64,
            phase: 0.into(),
            noise_phase: 0.into(),
            noise_x0: noise(),
            noise_x1: noise(),

            cur_freq_mul: freq_mul.load(LOAD_ORDERING),
            cur_freq_add: freq_add.load(LOAD_ORDERING),
            cur_amp_mul: amp_mul.load(LOAD_ORDERING),
            cur_noise_amp_mul: noise_amp_mul.load(LOAD_ORDERING),
            cur_noise_bw_scale: noise_bw_scale.load(LOAD_ORDERING),

            freq_mul,
            freq_add,
            amp_mul,
            noise_amp_mul,
            noise_bw_scale,

            inc_freq_mul: Arc::new(Atomic::new(0.001f64)),
            inc_freq_add: Arc::new(Atomic::new(1f64)),
            inc_amp_mul: Arc::new(Atomic::new(0.001f64)),
            inc_noise_amp_mul: Arc::new(Atomic::new(0.001f64)),
            inc_noise_bw_scale: Arc::new(Atomic::new(0.001f64)),
        }
    }

    pub fn interpolate_params(&mut self) {
        //interpolate
        self.cur_freq_mul = inc(
            self.cur_freq_mul,
            self.freq_mul.load(LOAD_ORDERING),
            self.inc_freq_mul.load(LOAD_ORDERING),
        );

        self.cur_freq_add = inc(
            self.cur_freq_add,
            self.freq_add.load(LOAD_ORDERING),
            self.inc_freq_add.load(LOAD_ORDERING),
        );
        self.cur_amp_mul = inc(
            self.cur_amp_mul,
            self.amp_mul.load(LOAD_ORDERING),
            self.inc_amp_mul.load(LOAD_ORDERING),
        );
        self.cur_noise_amp_mul = inc(
            self.cur_noise_amp_mul,
            self.noise_amp_mul.load(LOAD_ORDERING),
            self.inc_noise_amp_mul.load(LOAD_ORDERING),
        );
        self.cur_noise_bw_scale = inc(
            self.cur_noise_bw_scale,
            self.noise_bw_scale.load(LOAD_ORDERING),
            self.inc_noise_bw_scale.load(LOAD_ORDERING),
        );
    }

    pub fn synth(&mut self, freq: f64, sin_amp: f64, noise_energy: f64) -> f32 {
        self.interpolate_params();

        //apply transformations
        //should freq scaling affect noise bandwidth and offset?
        let freq = freq * self.cur_freq_mul + self.cur_freq_add;
        let sin_amp = self.cur_amp_mul * sin_amp;
        let noise_energy = noise_energy * self.cur_noise_amp_mul;

        //TODO if freq > 500 { 1 } else { 0.25 } * bw...
        let noise_bw = freq * self.cur_noise_bw_scale;

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
}

pub struct AtsSinNoiProcessor {
    current: Option<Arc<AtsData>>,
    data_recv: Receiver<Option<Arc<AtsData>>>,
    incr: ArcAtomic<usize>,
    offset: ArcAtomic<usize>,
    limit: ArcAtomic<usize>,
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
            self.current = c;
            cnt = cnt + 1;
            if cnt > DSP_RECV_MAX {
                break;
            }
        }

        let mut clear = || {
            for out in outputs[0].iter_mut() {
                *out = 0f32.into();
            }
        };

        if let Some(c) = &self.current {
            let with_noise = c.has_noise();
            let pmul = c.header.fra / c.header.dur;

            let start = self.offset.load(LOAD_ORDERING);
            let incr = self.incr.load(LOAD_ORDERING);
            let limit = self.limit.load(LOAD_ORDERING);
            let count = c.partials();
            if start >= count {
                clear();
                return;
            };
            let count = count - start;
            let count = count / incr + if (count % incr) > 0 { 1 } else { 0 };

            //total partials to synthesize
            let count = std::cmp::min(count, std::cmp::min(limit, self.synths.len()));

            if count == 0 {
                clear();
            } else {
                //end (exclusive) of partial data to synth
                let end = std::cmp::min(count * incr + start, c.partials());
                //indexes of partials (step_by later)
                let range = start..end;

                let synths = &mut self.synths[0..count];
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
                        f0[range.clone()].iter().step_by(incr),
                        f1[range.clone()].iter().step_by(incr)
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
        } else {
            clear();
        }
    }
}

fn set_clamp_bottom(a: &mut ArcAtomic<usize>, v: pd_sys::t_float, b: isize) {
    let v = std::cmp::max(b, v.floor() as isize) as usize;
    a.store(v, STORE_ORDERING);
}

pd_ext_macros::external! {
    #[name = "ats/sinnoi~"]
    pub struct AtsSinNoiExternal {
        data_send: SyncSender<Option<Arc<AtsData>>>,
        offset: ArcAtomic<usize>,
        incr: ArcAtomic<usize>,
        limit: ArcAtomic<usize>,
        handles: Box<[ParitalSynthHandle]>,
        post: Box<dyn PdPost>,
    }

    impl AtsSinNoiExternal {

        #[sel]
        pub fn ats_data(&mut self, key: pd_ext::symbol::Symbol) {
            let d = crate::cache::get(key);
            let _ = self.data_send.try_send(d);
            //TODO warn if empty?
        }

        #[sel]
        pub fn clear(&mut self) {
            let _ = self.data_send.send(None);
        }

        #[sel]
        pub fn offset(&mut self, v: pd_sys::t_float) {
            set_clamp_bottom(&mut self.offset, v, 0);
        }

        #[sel]
        pub fn incr(&mut self, v: pd_sys::t_float) {
            set_clamp_bottom(&mut self.incr, v, 1);
        }

        #[sel]
        pub fn limit(&mut self, v: pd_sys::t_float) {
            set_clamp_bottom(&mut self.limit, v, 0);
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
            let args = builder.creation_args();

            let mut partials = None;
            let mut offset = 0;
            let mut incr = 1;

            //get partial count
            if args.len() > 0 {
                if let Some(v) = args[0].get_int() {
                    partials = if v > 0 { Some(v) } else { None };
                }
                if args.len() >= 2 {
                    if let Some(v) = args[1].get_int() {
                        if v < 0 {
                            return Err("offset must be a positive integer".into());
                        }
                        offset = v;
                    }
                    if args.len() >= 3 {
                        if let Some(v) = args[2].get_int() {
                            if v < 1 {
                                return Err("increment must be an integer greater than 0".into());
                            }
                            incr = v;
                        }
                    }
                }
            }

            let offset = Arc::new(Atomic::new(offset as usize));
            let incr = Arc::new(Atomic::new(incr as usize));
            let limit = Arc::new(Atomic::new(std::usize::MAX));

            if let Some(partials) = partials {
                let mut synths = Vec::new();
                let mut handles = Vec::new();
                for _ in 0..partials {
                    let (h, s) = ParitalSynthHandle::new();
                    handles.push(h);
                    synths.push(s);
                }

                Ok(
                    (
                        Self {
                            data_send,
                            handles: handles.into(),
                            offset: offset.clone(),
                            incr: incr.clone(),
                            limit: limit.clone(),
                            post: builder.poster()
                        },
                        Box::new(AtsSinNoiProcessor {
                            current: None,
                            data_recv,
                            offset,
                            incr,
                            limit,
                            synths: synths.into(),
                        })
                    )
                )
            } else {
                Err("first argument must be a non zero partial count".into())
            }
        }
    }
}

fn lerp(x0: f64, x1: f64, frac: f64) -> f64 {
    x0 + (x1 - x0) * frac
}

fn inc(cur: f64, dest: f64, inc: f64) -> f64 {
    //if within inc of dest, return dest
    if cur == dest || (cur - dest).abs() <= inc {
        dest
    } else if cur < dest {
        cur + inc
    } else {
        cur - inc
    }
}
