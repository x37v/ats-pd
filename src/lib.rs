use ats_sys::{ANARGS, ATS_HEADER};
use byteorder::{LittleEndian, ReadBytesExt};
use clap::{App, AppSettings, Arg};
use pd_ext::builder::ControlExternalBuilder;
use pd_ext::clock::Clock;
use pd_ext::external::ControlExternal;
use pd_ext::outlet::{OutletSend, OutletType};
use pd_ext::post::PdPost;
use pd_ext::symbol::Symbol;
use pd_ext_macros::external;
use std::convert::TryInto;
use std::ffi::CString;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::os::raw::c_int;
use std::path::{Path, PathBuf};
use std::slice;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};

lazy_static::lazy_static! {
    static ref SAMPLE_RATE: Symbol = "sample_rate".try_into().unwrap();
    static ref FRAME_SIZE: Symbol = "frame_samps".try_into().unwrap();
    static ref WINDOW_SIZE: Symbol = "window_samps".try_into().unwrap();
    static ref PARTIAL_COUNT: Symbol = "partial_count".try_into().unwrap();
    static ref FRAME_COUNT: Symbol = "frame_count".try_into().unwrap();
    static ref AMP_MAX: Symbol = "amp_max".try_into().unwrap();
    static ref FREQ_MAX: Symbol = "freq_max".try_into().unwrap();
    static ref DUR_SECONDS: Symbol = "dur_sec".try_into().unwrap();
    static ref FILE_TYPE: Symbol = "file_type".try_into().unwrap();

    static ref PLOT_INFO_TRACKS: Symbol = "track_count".try_into().unwrap();
    static ref PLOT_INFO_BANDS: Symbol = "noise_band".try_into().unwrap();

    static ref PLOT_TRACK: Symbol = "track_point".try_into().unwrap();
    static ref PLOT_NOISE: Symbol = "noise_point".try_into().unwrap();
    //indicate if we're actively dumping
    static ref DUMPING: Symbol = "dumping".try_into().unwrap();
}

const NOISE_BANDS: usize = 25;
static NOISE_BAND_EDGES: &[f64; NOISE_BANDS + 1] = &[
    0.0, 100.0, 200.0, 300.0, 400.0, 510.0, 630.0, 770.0, 920.0, 1080.0, 1270.0, 1480.0, 1720.0,
    2000.0, 2320.0, 2700.0, 3150.0, 3700.0, 4400.0, 5300.0, 6400.0, 7700.0, 9500.0, 12000.0,
    15500.0, 20000.0,
];

enum AtsFileType {
    AmpFreq = 1,
    AmpFreqPhase = 2,
    AmpFreqNoise = 3,
    AmpFreqPhaseNoise = 4,
}

struct AtsFile {
    pub header: ATS_HEADER,
    pub frames: Vec<Vec<Peak>>,
    pub noise: Option<Vec<[f64; NOISE_BANDS]>>,
    pub file_type: AtsFileType,
}

fn stringify<E: std::fmt::Display>(x: E) -> String {
    format!("error code: {}", x)
}

fn to_cstring(p: PathBuf) -> Result<CString, String> {
    let s = p.to_str();
    if let Some(s) = s {
        if let Ok(s) = CString::new(s) {
            Ok(s)
        } else {
            Err("cannot create Cstring".into())
        }
    } else {
        Err("cannot create str".into())
    }
}

impl AtsFile {
    pub fn try_read<P: AsRef<std::path::Path>>(path: P) -> std::io::Result<Self> {
        let mut header: std::mem::MaybeUninit<ATS_HEADER> = std::mem::MaybeUninit::uninit();
        let mut file = File::open(path)?;
        unsafe {
            let s = slice::from_raw_parts_mut(
                &mut header as *mut _ as *mut u8,
                std::mem::size_of::<ATS_HEADER>(),
            );
            file.read_exact(s)?;
            let header = header.assume_init();

            if header.mag != 123f64 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "magic number does not match",
                ));
            }
            let file_type = match header.typ as usize {
                1 => AtsFileType::AmpFreq,
                2 => AtsFileType::AmpFreqPhase,
                3 => AtsFileType::AmpFreqNoise,
                4 => AtsFileType::AmpFreqPhaseNoise,
                _ => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!("{} type ATS files not supported yet", header.typ),
                    ))
                }
            };

            let mut frames = Vec::new();
            let mut noise = Vec::new();
            for _f in 0..header.fra as usize {
                //skip frame time
                file.seek(SeekFrom::Current(std::mem::size_of::<f64>() as i64))?;

                let mut frame_peaks = Vec::new();

                for _p in 0..header.par as usize {
                    let mut amp_freq = [0f64; 2];
                    file.read_f64_into::<LittleEndian>(&mut amp_freq)?;
                    let mut peak = Peak {
                        amp: amp_freq[0],
                        freq: amp_freq[1],
                        phase: None,
                    };
                    match file_type {
                        AtsFileType::AmpFreqPhase | AtsFileType::AmpFreqPhaseNoise => {
                            peak.phase = Some(file.read_f64::<LittleEndian>()?)
                        }
                        _ => (),
                    }
                    frame_peaks.push(peak);
                }
                match file_type {
                    AtsFileType::AmpFreqNoise | AtsFileType::AmpFreqPhaseNoise => {
                        let mut nframe = [0f64; 25];
                        file.read_f64_into::<LittleEndian>(&mut nframe)?;
                        noise.push(nframe);
                    }
                    _ => (),
                }
                frames.push(frame_peaks);
            }

            let noise = if noise.len() != 0 { Some(noise) } else { None };
            Ok(Self {
                header,
                frames,
                noise,
                file_type,
            })
        }
    }
}

struct Peak {
    amp: f64,
    freq: f64,
    phase: Option<f64>,
}

external! {
    pub struct AtsDump {
        current: Option<AtsFile>,
        outlet: Box<dyn OutletSend>,
        clock: Clock,
        post: Box<dyn PdPost>,
        waiting: AtomicUsize,
        file_send: Sender<std::io::Result<AtsFile>>,
        file_recv: Receiver<std::io::Result<AtsFile>>,
    }

    impl ControlExternal for AtsDump {
        fn new(builder: &mut dyn ControlExternalBuilder<Self>) -> Self {
            let outlet = builder.new_message_outlet(OutletType::AnyThing);
            let clock = Clock::new(builder.obj(), atsdump_poll_done_trampoline);
            let (file_send, file_recv) = channel();
            let post = builder.poster();
            Self {
                outlet,
                current: None,
                clock,
                post,
                waiting: Default::default(),
                file_send,
                file_recv
            }
        }
    }

    impl AtsDump {
        fn send_noise_bands(&self) {
            for i in 0..NOISE_BANDS {
                let x0 = NOISE_BAND_EDGES[i];
                let x1 = NOISE_BAND_EDGES[i + 1];
                self.outlet.send_anything(*PLOT_INFO_BANDS, &[i.into(), x0.into(), x1.into()]);
            }
        }

        fn send_tracks(&self, f: &AtsFile) {
            //data is in frames, each frame has the same number of tracks
            //we output track index, frame index, freq, amp
            for (i, frame) in f.frames.iter().enumerate() {
                for (j, track) in frame.iter().enumerate() {
                    self.outlet.send_anything(*PLOT_TRACK, &[j.into(), i.into(), track.freq.into(), track.amp.into()]);
                }
            }
        }

        fn send_noise(&self, f: &AtsFile) {
            if let Some(n) = &f.noise {
                for (i, frame) in n.iter().enumerate() {
                    for (j, energy) in frame.iter().enumerate() {
                        self.outlet.send_anything(*PLOT_NOISE, &[j.into(), i.into(), (*energy).into()]);
                    }
                }
            }
        }

        fn send_file_info(&self, f: &AtsFile) {
            self.outlet.send_anything(*FILE_TYPE, &[f.header.typ.into()]);
            self.outlet.send_anything(*SAMPLE_RATE, &[f.header.sr.into()]);
            self.outlet.send_anything(*DUR_SECONDS, &[f.header.dur.into()]);
            self.outlet.send_anything(*FRAME_SIZE, &[f.header.fs.into()]);
            self.outlet.send_anything(*WINDOW_SIZE, &[f.header.ws.into()]);
            self.outlet.send_anything(*PARTIAL_COUNT, &[f.header.par.into()]);
            self.outlet.send_anything(*FRAME_COUNT, &[f.header.fra.into()]);
            self.outlet.send_anything(*AMP_MAX, &[f.header.ma.into()]);
            self.outlet.send_anything(*FREQ_MAX, &[f.header.mf.into()]);
        }

        #[bang]
        pub fn bang(&mut self) {
            self.outlet.send_anything(*DUMPING, &[1.into()]);
            if let Some(f) = &self.current {
                self.send_noise_bands();
                self.send_file_info(f);
                self.outlet.send_anything(*PLOT_INFO_TRACKS, &[f.header.par.into(), f.header.fra.into()]);
                self.send_tracks(f);
                self.send_noise(f);
            } else {
                self.outlet.send_anything(*FILE_TYPE, &[0f32.into()]);
                self.outlet.send_anything(*PLOT_INFO_TRACKS, &[0f32.into(), 0f32.into()]);
            }
            self.outlet.send_anything(*DUMPING, &[0.into()]);
        }

        #[sel]
        pub fn open(&mut self, filename: Symbol) {
            let s = self.file_send.clone();
            self.waiting.fetch_add(1, Ordering::SeqCst);
            std::thread::spawn(move || s.send(AtsFile::try_read(filename)));
            self.clock.delay(1f64);
        }


        fn extract_args(&self, cmd_name: &str, args: &[pd_ext::atom::Atom]) -> Result<(String, ANARGS), String> {
            let v = args.iter().map(|a| (*a).try_into()).collect::<Result<Vec<String>, _>>()?;
            let mut app = App::new(cmd_name)
                .setting(AppSettings::ArgRequiredElseHelp)
                .setting(AppSettings::NoBinaryName)
                .setting(AppSettings::ColorNever)
                .setting(AppSettings::DisableHelpSubcommand)
                .setting(AppSettings::DisableHelpFlags)
                .setting(AppSettings::DisableVersion)
                .setting(AppSettings::DeriveDisplayOrder)
                .arg(Arg::with_name("source")
                    .index(1)
                    .required(true)
                    .help("the thing you want to analyize")
                )
                //"\t -e duration (%f seconds or end)\n"         \
                .arg(Arg::with_name("duration")
                    .short("e")
                    .long("duration")
                    .takes_value(true)
                )
                //"\t -l lowest frequency (%f Hertz)\n"          \
                .arg(Arg::with_name("lowest_frequency")
                    .short("l")
                    .long("lowest_freq")
                    .takes_value(true)
                )
                //"\t -H highest frequency (%f Hertz)\n"         \
                .arg(Arg::with_name("highest_frequency")
                    .short("H")
                    .long("highest_freq")
                    .takes_value(true)
                )
                //"\t -d frequency deviation (%f of partial freq.)\n"    \
                .arg(Arg::with_name("frequency_deviation")
                    .short("d")
                    .long("freq_dev")
                    .takes_value(true)
                )
                //"\t -c window cycles (%d cycles)\n"                           \
                .arg(Arg::with_name("window_cycles")
                    .short("c")
                    .long("window_cycles")
                    .takes_value(true)
                )
                //"\t -w window type (type: %d)\n"                              \
                .arg(Arg::with_name("window_type")
                    .short("w")
                    .long("window_type")
                    .takes_value(true)
                    //XXX options
                )
                //"\t\t(Options: 0=BLACKMAN, 1=BLACKMAN_H, 2=HAMMING, 3=VONHANN)\n" \
                //"\t -h hop size (%f of window size)\n"                        \
                .arg(Arg::with_name("hop_size")
                    .short("h")
                    .long("hop_size")
                    .takes_value(true)
                )
                //"\t -m lowest magnitude (%f)\n"                               \
                .arg(Arg::with_name("lowest_magnitude")
                    .short("m")
                    .long("lowest_mag")
                    .takes_value(true)
                    .help("float")
                )
                //"\t -t track length (%d frames)\n"                            \
                .arg(Arg::with_name("track_length")
                    .short("t")
                    .long("track_len")
                    .takes_value(true)
                    .help("int frames")
                )
                //"\t -s min. segment length (%d frames)\n"                     \
                //"\t -g min. gap length (%d frames)\n"                         \
                //"\t -T SMR threshold (%f dB SPL)\n"                           \
                //"\t -S min. segment SMR (%f dB SPL)\n"                        \
                //"\t -P last peak contribution (%f of last peak's parameters)\n" \
                //"\t -M SMR contribution (%f)\n"                               \
                //"\t -F File Type (type: %d)\n"                                \
                //"\t\t(Options: 1=amp.and freq. only, 2=amp.,freq. and phase, 3=amp.,freq. and residual, 4=amp.,freq.,phase, and residual)\n\n",
                ;
            let matches = app.clone().get_matches_from_safe(v);

            match matches {
                Ok(m) => {
                    let mut oargs: ANARGS = Default::default();
                    let source = m.value_of("source").unwrap().into();
                    if let Some(v) = m.value_of("duration") {
                        oargs.duration = v.parse::<f32>().map_err(stringify)?;
                    }
                    if let Some(v) = m.value_of("lowest_frequency") {
                        oargs.lowest_freq = v.parse::<f32>().map_err(stringify)?;
                    }
                    if let Some(v) = m.value_of("highest_frequency") {
                        oargs.highest_freq = v.parse::<f32>().map_err(stringify)?;
                    }
                    if let Some(v) = m.value_of("frequency_deviation") {
                        oargs.freq_dev = v.parse::<f32>().map_err(stringify)?;
                    }
                    if let Some(v) = m.value_of("window_cycles") {
                        oargs.win_cycles = v.parse::<c_int>().map_err(stringify)?;
                    }
                    if let Some(v) = m.value_of("window_type") {
                        oargs.win_type = v.parse::<c_int>().map_err(stringify)?;
                    }
                    if let Some(v) = m.value_of("hop_size") {
                        oargs.hop_size = v.parse::<f32>().map_err(stringify)?;
                    }
                    if let Some(v) = m.value_of("track_length") {
                        oargs.track_len = v.parse::<c_int>().map_err(stringify)?;
                    }
                    Ok((source, oargs))
                },
                Err(m) => {
                    let mut help = Vec::new();
                    let _ = app.write_long_help(&mut help);
                    let help = String::from_utf8(help);
                    if let Ok(help) = help {
                        Err(format!("{} {}", m.message, help))
                    } else {
                        Err(m.message)
                    }
                }
            }
        }

        #[sel]
        pub fn anal_file(&mut self, args: &[pd_ext::atom::Atom]) {
            let args = self.extract_args("anal_file", args);
            match args {
                Ok((f, mut args)) => {
                    if !Path::new(&f).exists() {
                        self.post.post_error(format!("file does not exist: {}", f));
                    } else {
                        if let Ok(dir) = tempfile::tempdir() {
                            let outpath = dir.path().join("out.ats");
                            let infile = CString::new(f.clone()).unwrap().into_raw();
                            let outfile = to_cstring(outpath.clone());
                            let resfile = to_cstring(dir.path().join("atsa_res.wav"));
                            if outfile.is_err() || resfile.is_err() {
                                self.post.post_error("cannot get out or resfile paths".into());
                            } else {
                                let outfile = outfile.unwrap().into_raw();
                                let resfile = resfile.unwrap().into_raw();
                                unsafe {
                                    let v = ats_sys::main_anal(infile, outfile, &mut args, resfile);
                                    //cleanup constructed cstring
                                    let _ = CString::from_raw(infile);
                                    let _ = CString::from_raw(outfile);
                                    let _ = CString::from_raw(resfile);
                                    match v {
                                        0 => self.post.post(format!("analyized file {}", f)),
                                        e @ _ => self.post.post_error(format!("failed to analyize file: {} with error num: {}", f, e))
                                    };
                                }
                            }
                        } else {
                            self.post.post_error("failed to create tempdir".into());
                        }
                    }
                },
                Err(err) => {
                    self.post.post_error(err);
                }
            }
        }

        #[sel]
        pub fn anal(&mut self, _filename: Symbol) {
            let infile = CString::new("/tmp/test.aif").unwrap().into_raw();
            let outfile = CString::new("/tmp/test.ats").unwrap().into_raw();
            let resfile = CString::new("/tmp/atsa_res.wav").unwrap().into_raw();

            let mut args = Default::default();
            unsafe {
                let v = ats_sys::main_anal(infile, outfile, &mut args, resfile);
                let _ = CString::from_raw(infile);
                let _ = CString::from_raw(outfile);
                let _ = CString::from_raw(resfile);
                self.post.post(format!("anal {}", v));
            }
        }

        #[tramp]
        pub fn poll_done(&mut self) {
            let mut waiting = 1;
            if let Ok(res) = self.file_recv.try_recv() {
                waiting = self.waiting.fetch_sub(1, Ordering::SeqCst) - 1;
                self.current = match res {
                    Ok(f) => {
                        self.post.post(format!("read"));
                        Some(f)
                    },
                    Err(err) => {
                        self.post.post_error(format!("error {}", err));
                        None
                    }
                };
                self.bang();
            }
            if waiting != 0 {
                self.clock.delay(1f64);
            }
        }
    }
}
