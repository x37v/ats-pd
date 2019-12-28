use ats_sys::ATS_HEADER;
use byteorder::{LittleEndian, ReadBytesExt};
use pd_ext::builder::ControlExternalBuilder;
use pd_ext::external::ControlExternal;
use pd_ext::outlet::{OutletSend, OutletType};
use pd_ext::pd;
use pd_ext::symbol::Symbol;
use pd_ext_macros::external;
use std::convert::TryInto;
use std::ffi::CString;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::slice;

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
    }

    impl ControlExternal for AtsDump {
        fn new(builder: &mut dyn ControlExternalBuilder<Self>) -> Self {
            let outlet = builder.new_message_outlet(OutletType::AnyThing);
            Self {
                outlet,
                current: None
            }
        }
    }

    impl AtsDump {
        fn post(&self, v: String) {
            pd::post(CString::new(format!("atsdump: {}", v)).unwrap());
        }

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
            self.current = match AtsFile::try_read(filename) {
                Ok(f) => {
                    self.post(format!("read {}", filename));
                    Some(f)
                },
                Err(err) => {
                    self.post(format!("error {}", err));
                    None
                }
            };
            self.bang();
        }
    }
}
