use ats_sys::ATS_HEADER;
use byteorder::{LittleEndian, ReadBytesExt};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::slice;

pub const NOISE_BANDS: usize = 25;
pub static NOISE_BAND_EDGES: &[f64; NOISE_BANDS + 1] = &[
    0.0, 100.0, 200.0, 300.0, 400.0, 510.0, 630.0, 770.0, 920.0, 1080.0, 1270.0, 1480.0, 1720.0,
    2000.0, 2320.0, 2700.0, 3150.0, 3700.0, 4400.0, 5300.0, 6400.0, 7700.0, 9500.0, 12000.0,
    15500.0, 20000.0,
];

pub enum AtsDataType {
    AmpFreq = 1,
    AmpFreqPhase = 2,
    AmpFreqNoise = 3,
    AmpFreqPhaseNoise = 4,
}

pub struct Peak {
    pub amp: f64,
    pub freq: f64,
    pub noise_energy: Option<f64>,
    pub phase: Option<f64>,
}

pub struct AtsData {
    pub header: ATS_HEADER,
    pub frames: Box<[Box<[Peak]>]>,
    pub noise: Option<Box<[[f64; NOISE_BANDS]]>>,
    pub file_type: AtsDataType,
    pub source: String,
}

fn energy_rms(value: f64, window_size: f64) -> f64 {
    (value / (window_size * 0.04f64)).sqrt()
}

impl AtsData {
    pub fn try_read<P: AsRef<std::path::Path>>(path: P) -> std::io::Result<Self> {
        let mut header: std::mem::MaybeUninit<ATS_HEADER> = std::mem::MaybeUninit::uninit();
        let source = path.as_ref().to_string_lossy().into_owned();
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
                1 => AtsDataType::AmpFreq,
                2 => AtsDataType::AmpFreqPhase,
                3 => AtsDataType::AmpFreqNoise,
                4 => AtsDataType::AmpFreqPhaseNoise,
                _ => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!("{} type ATS files not supported yet", header.typ),
                    ))
                }
            };

            let mut frames = Vec::new();
            let mut noise = Vec::new();
            let mut partialband: Vec<usize> = std::iter::repeat(0usize)
                .take(header.par as usize)
                .collect();

            let bands: Vec<(usize, f64, f64)> = NOISE_BAND_EDGES[0..NOISE_BANDS]
                .iter()
                .zip(NOISE_BAND_EDGES[1..].iter())
                .enumerate()
                .map(|v| (v.0, *((v.1).0), *((v.1).1)))
                .collect();
            for _f in 0..header.fra as usize {
                let mut band_amp_sum = [0f64; NOISE_BANDS];

                //skip frame time
                file.seek(SeekFrom::Current(std::mem::size_of::<f64>() as i64))?;

                let mut frame_peaks = Vec::new();

                for p in 0..header.par as usize {
                    let mut amp_freq = [0f64; 2];
                    file.read_f64_into::<LittleEndian>(&mut amp_freq)?;
                    let mut peak = Peak {
                        amp: amp_freq[0],
                        freq: amp_freq[1],
                        noise_energy: None,
                        phase: None,
                    };

                    //find noise band
                    let band = bands
                        .iter()
                        .find(|&b| b.1 <= peak.freq && peak.freq < b.2)
                        .unwrap_or(&(NOISE_BANDS - 1, 0f64, 0f64))
                        .0;
                    partialband[p] = band;
                    band_amp_sum[band] += peak.amp;

                    match file_type {
                        AtsDataType::AmpFreqPhase | AtsDataType::AmpFreqPhaseNoise => {
                            peak.phase = Some(file.read_f64::<LittleEndian>()?)
                        }
                        _ => (),
                    }
                    frame_peaks.push(peak);
                }
                match file_type {
                    AtsDataType::AmpFreqNoise | AtsDataType::AmpFreqPhaseNoise => {
                        let mut nframe = [0f64; 25];
                        file.read_f64_into::<LittleEndian>(&mut nframe)?;

                        //compute energy per parital
                        for (p, b) in frame_peaks.iter_mut().zip(partialband.iter()) {
                            let s = band_amp_sum[*b];
                            let e = nframe[*b];
                            p.noise_energy = Some(if s > 0f64 {
                                energy_rms(p.amp * e / s, header.ws)
                            } else {
                                0f64
                            });
                        }

                        //store
                        noise.push(nframe);
                    }
                    _ => (),
                }
                frames.push(frame_peaks.into_boxed_slice());
            }

            let noise = if noise.len() != 0 {
                Some(noise.into_boxed_slice())
            } else {
                None
            };
            Ok(Self {
                header,
                frames: frames.into_boxed_slice(),
                noise,
                file_type,
                source,
            })
        }
    }
}
