use ats_sys::ANARGS;
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
use std::os::raw::c_int;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::sync::Mutex;

use crate::data::AtsData;

external! {
    #[name="ats/data"]
    pub struct AtsDataExternal {
        current: Option<(Symbol, Arc<AtsData>)>,
        outlet: Box<dyn OutletSend>,
        clock: Clock,
        post: Box<dyn PdPost>,
        waiting: AtomicUsize,
        file_send: Sender<Result<(AtsData, String), String>>,
        file_recv: Receiver<Result<(AtsData, String), String>>,
    }

    impl ControlExternal for AtsDataExternal {
        fn new(builder: &mut dyn ControlExternalBuilder<Self>) -> Self {
            let outlet = builder.new_message_outlet(OutletType::AnyThing);
            let clock = Clock::new(builder.obj(), atsdataexternal_poll_done_trampoline);
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

    impl AtsDataExternal {
        fn send_file_info(&self, f: &AtsData) {
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
            if let Some((k, f)) = &self.current {
                self.send_file_info(f);
                self.outlet.send_anything(*DATA_KEY, &[(*k).into()]);
            } else {
                self.outlet.send_anything(*FILE_TYPE, &[0f32.into()]);
            }
        }

        #[sel]
        pub fn open(&mut self, filename: Symbol) {
            self.queue_job(move || AtsData::try_read(filename).map_err(stringify).map(|r| (r, filename.into())))
        }

        #[sel]
        pub fn help(&mut self) {
            let mut app = create_app("anal_file");
            let mut help = Vec::new();
            if app.write_long_help(&mut help).is_ok() {
                let help = String::from_utf8(help);
                if let Ok(help) = help {
                    self.post.post(help);
                }
            }
        }

        #[sel]
        pub fn anal_file(&mut self, args: &[pd_ext::atom::Atom]) {
            let args = args
                .iter()
                .map(|a| (*a).try_into())
                .collect::<Result<Vec<String>, _>>();
            if let Ok(args) = args {
                self.queue_job(|| {
                    let args = extract_args("anal_file", args);
                    match args {
                        Ok((f, mut args)) => {
                            if !Path::new(&f).exists() {
                                Err(format!("file does not exist: {}", f))
                            } else {
                                if let Ok(dir) = tempfile::tempdir() {
                                    //create temp path, based on original file name if possible
                                    let outpath = dir.path().join(format!("{}.ats", Path::new(&f).file_stem().unwrap_or(std::ffi::OsStr::new("out")).to_string_lossy()));
                                    let infile = CString::new(f.clone()).unwrap().into_raw();
                                    let outfile = to_cstring(outpath.clone());
                                    //ATS seems to always want the residual file in the same place
                                    //let resfile = to_cstring(dir.path().join("atsa_res.wav"));
                                    let mut resfile = ats_sys::ATSA_RES_FILE.to_vec();
                                    resfile.retain(|&x| x != b'\0'); // remove Nul
                                    let resfile = CString::new(resfile).unwrap();
                                    let resfile:Result<CString, String> = Ok(resfile);
                                    if outfile.is_err() || resfile.is_err() {
                                        Err("cannot get out or resfile paths".into())
                                    } else {
                                        let outfile = outfile.unwrap().into_raw();
                                        let resfile = resfile.unwrap().into_raw();
                                        unsafe {
                                            let v = {
                                                //all analysis uses the same residual file so we
                                                //must lock
                                                let _ = ANAL_MUTEX.lock().unwrap();
                                                ats_sys::main_anal(infile, outfile, &mut args, resfile)
                                            };
                                            //cleanup constructed cstring
                                            let _ = CString::from_raw(infile);
                                            let _ = CString::from_raw(outfile);
                                            let _ = CString::from_raw(resfile);
                                            match v {
                                                0 => AtsData::try_read(outpath).map_err(stringify).map(|r| (r, f)),
                                                e @ _ => Err(format!("failed to analyize file: {} with error num: {}", f, e))
                                            }
                                        }
                                    }
                                } else {
                                    Err("failed to create tempdir".into())
                                }
                            }
                        },
                        Err(e) => {
                            Err(e)
                        }
                    }
                });
            } else {
                self.post.post_error("failed to convert args to a string array".into());
            }
        }

        fn queue_job<F: 'static + Send + FnOnce() -> Result<(AtsData, String), String>>(&mut self, job: F) {
            let s = self.file_send.clone();
            self.waiting.fetch_add(1, Ordering::SeqCst);
            std::thread::spawn(move || s.send(job()));
            self.clock.delay(1f64);
        }

        #[tramp]
        pub fn poll_done(&mut self) {
            let mut waiting = 1;
            if let Ok(res) = self.file_recv.try_recv() {
                waiting = self.waiting.fetch_sub(1, Ordering::SeqCst) - 1;
                self.current = match res {
                    Ok((f, filename)) => {
                        self.post.post(format!("read {}", filename));
                        //store in cache
                        let c = Arc::new(f);
                        let k = crate::cache::insert(c.clone());
                        Some((k, c))
                    },
                    Err(err) => {
                        self.post.post_error(err);
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

    pub static ref DATA_KEY: Symbol = "ats_data".try_into().unwrap();
    static ref ANAL_MUTEX: Mutex<()> = Mutex::new(());
}

fn create_app(cmd_name: &str) -> App {
    App::new(cmd_name)
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
        //"\t -e duration (%f seconds or end)\n"
        .arg(Arg::with_name("duration")
            .short("e")
            .long("duration")
            .takes_value(true)
            .help("float seconds, defaults to the whole soundfile")
        )
        //"\t -l lowest frequency (%f Hertz)\n"
        .arg(Arg::with_name("lowest_frequency")
            .short("l")
            .long("lowest_freq")
            .takes_value(true)
            .help("float Hertz")
        )
        //"\t -H highest frequency (%f Hertz)\n"
        .arg(Arg::with_name("highest_frequency")
            .short("H")
            .long("highest_freq")
            .takes_value(true)
            .help("float Hertz")
        )
        //"\t -d frequency deviation (%f of partial freq.)\n"
        .arg(Arg::with_name("frequency_deviation")
            .short("d")
            .long("freq_dev")
            .takes_value(true)
            .help("float of partial freq")
        )
        //"\t -c window cycles (%d cycles)\n"
        .arg(Arg::with_name("window_cycles")
            .short("c")
            .long("window_cycles")
            .takes_value(true)
            .help("int number of cycles")
        )
        //"\t -w window type (type: %d)\n"
        .arg(Arg::with_name("window_type")
            .short("w")
            .long("window_type")
            .takes_value(true)
            .possible_values(&["0","1","2","3"])
            .help("0=BLACKMAN, 1=BLACKMAN_H, 2=HAMMING, 3=VONHANN")
        )
        //"\t -h hop size (%f of window size)\n"
        .arg(Arg::with_name("hop_size")
            .short("h")
            .long("hop_size")
            .takes_value(true)
            .help("float, portion of window size")
        )
        //"\t -m lowest magnitude (%f)\n"
        .arg(Arg::with_name("lowest_magnitude")
            .short("m")
            .long("lowest_mag")
            .takes_value(true)
            .help("float")
        )
        //"\t -t track length (%d frames)\n"
        .arg(Arg::with_name("track_length")
            .short("t")
            .long("track_len")
            .takes_value(true)
            .help("int frames")
        )
        //"\t -s min. segment length (%d frames)\n"
        .arg(Arg::with_name("min_segment_length")
            .short("s")
            .long("min_seg_len")
            .takes_value(true)
            .help("int frames")
        )
        //"\t -g min. gap length (%d frames)\n"
        .arg(Arg::with_name("min_gap_length")
            .short("g")
            .long("min_gap_len")
            .takes_value(true)
            .help("int frames")
        )
        //"\t -T SMR threshold (%f dB SPL)\n"
        .arg(Arg::with_name("smr_threshold")
            .short("T")
            .long("smr_thresh")
            .takes_value(true)
            .help("float dB SPL")
        )
        //"\t -S min. segment SMR (%f dB SPL)\n"
        .arg(Arg::with_name("min_segment_smr")
            .short("S")
            .long("min_seg_smr")
            .takes_value(true)
            .help("float dB SPL")
        )
        //"\t -P last peak contribution (%f of last peak's parameters)\n"
        .arg(Arg::with_name("last_peak_contribution")
            .short("P")
            .long("last_peak_cont")
            .takes_value(true)
            .help("float, of last peak's parameters")
        )
        //"\t -M SMR contribution (%f)\n"
        .arg(Arg::with_name("smr_contribution")
            .short("M")
            .long("smr_cont")
            .takes_value(true)
            .help("float")
        )
        //"\t -F File Type (type: %d)\n"
        //"\t\t(Options: 1=amp.and freq. only, 2=amp.,freq. and phase, 3=amp.,freq. and residual, 4=amp.,freq.,phase, and residual)\n\n",
        .arg(Arg::with_name("file_type")
            .short("F")
            .long("file_type")
            .takes_value(true)
            .possible_values(&["1", "2", "3", "4"])
            .help("Options: 1=amp.and freq. only, 2=amp.,freq. and phase, 3=amp.,freq. and residual, 4=amp.,freq.,phase, and residual")
        )
}

fn extract_args(cmd_name: &str, args: Vec<String>) -> Result<(String, ANARGS), String> {
    let mut app = create_app(cmd_name);
    let matches = app.clone().get_matches_from_safe(args);

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
            if let Some(v) = m.value_of("min_segment_length") {
                oargs.min_seg_len = v.parse::<c_int>().map_err(stringify)?;
            }
            if let Some(v) = m.value_of("min_gap_length") {
                oargs.min_gap_len = v.parse::<c_int>().map_err(stringify)?;
            }
            if let Some(v) = m.value_of("smr_threshold") {
                oargs.SMR_thres = v.parse::<f32>().map_err(stringify)?;
            }
            if let Some(v) = m.value_of("min_segment_smr") {
                oargs.min_seg_SMR = v.parse::<f32>().map_err(stringify)?;
            }
            if let Some(v) = m.value_of("last_peak_contribution") {
                oargs.last_peak_cont = v.parse::<f32>().map_err(stringify)?;
            }
            if let Some(v) = m.value_of("smr_contribution") {
                oargs.SMR_cont = v.parse::<f32>().map_err(stringify)?;
            }
            if let Some(v) = m.value_of("file_type") {
                oargs.type_ = v.parse::<c_int>().map_err(stringify)?;
            }
            Ok((source, oargs))
        }
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

fn stringify<E: std::fmt::Display>(x: E) -> String {
    format!("error code: {}", x)
}
