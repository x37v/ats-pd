use ats_sys::ATS_HEADER;
use byteorder::{LittleEndian, ReadBytesExt};
use pd_ext::builder::ControlExternalBuilder;
use pd_ext::external::ControlExternal;
use pd_ext::outlet::{OutletSend, OutletType};
use pd_ext::pd;
use pd_ext::symbol::Symbol;
use pd_ext_macros::external;
use std::ffi::CString;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::rc::Rc;
use std::slice;

const NOISE_BANDS: usize = 25;

enum AtsFileType {
    AmpFreq = 1,
    AmpFreqPhase = 2,
    AmpFreqNoise = 3,
    AmpFreqPhaseNoise = 4,
}

struct AtsFile {
    header: ATS_HEADER,
    peaks: Vec<Vec<Peak>>,
    noise: Option<Vec<[f64; NOISE_BANDS]>>,
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
            Self { outlet, current: None }
        }
    }

    impl AtsDump {
        fn post(&self, v: String) {
            pd::post(CString::new(format!("atsdump: {}", v)).unwrap());
        }

        #[bang] //indicates that a bang in Pd should call this
        pub fn bang(&mut self) {
            //pd::post(CString::new("Hello world !!").unwrap());
        }

        #[sel]
        pub fn open(&mut self, filename: Symbol) {
            match self.read_file(&filename) {
                Ok(_) => self.post(format!("read {}", filename)),
                Err(err) => self.post(format!("error {}", err))
            }
        }

        fn read_file(&mut self, filename: &Symbol) -> std::io::Result<AtsFile> {
            let mut file = File::open(filename)?;
            let mut header: std::mem::MaybeUninit<ATS_HEADER> = std::mem::MaybeUninit::uninit();
            unsafe {
                let s = slice::from_raw_parts_mut(&mut header as *mut _ as *mut u8, std::mem::size_of::<ATS_HEADER>());
                file.read_exact(s)?;
                let header = header.assume_init();

                if header.mag != 123f64 {
                    return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "magic number does not match"));
                }
                let typ = match header.typ as usize {
                    1 => AtsFileType::AmpFreq,
                    2 => AtsFileType::AmpFreqPhase,
                    3 => AtsFileType::AmpFreqNoise,
                    4 => AtsFileType::AmpFreqPhaseNoise,
                    _ => return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, format!("{} type ATS files not supported yet", header.typ)))
                };

                let mut peaks = Vec::new();
                let mut noise = Vec::new();
                for f in 0..header.fra as usize {
                    //skip frame time
                    file.seek(SeekFrom::Current(std::mem::size_of::<f64>() as i64))?;

                    let mut frame_peaks = Vec::new();

                    for p in 0..header.par as usize {
                        let mut amp_freq = [0f64; 2];
                        file.read_f64_into::<LittleEndian>(&mut amp_freq)?;
                        let mut peak = Peak {
                            amp: amp_freq[0],
                            freq: amp_freq[1],
                            phase: None
                        };
                        match typ {
                            AtsFileType::AmpFreqPhase | AtsFileType::AmpFreqPhaseNoise => peak.phase = Some(file.read_f64::<LittleEndian>()?),
                            _ => ()
                        }
                        frame_peaks.push(peak);
                    }
                    match typ {
                        AtsFileType::AmpFreqNoise | AtsFileType::AmpFreqPhaseNoise => {
                            let mut nframe = [0f64; 25];
                            file.read_f64_into::<LittleEndian>(&mut nframe)?;
                            noise.push(nframe);
                        }
                        _ => ()
                    }
                    peaks.push(frame_peaks);
                }

                let noise = if noise.len() != 0 {
                    Some(noise)
                } else {
                    None
                };
                Ok(AtsFile{
                    header,
                    peaks,
                    noise
                })
            }
        }

    }
}
