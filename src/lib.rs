use ats_sys::ATS_HEADER;
use pd_ext::builder::ControlExternalBuilder;
use pd_ext::external::ControlExternal;
use pd_ext::outlet::{OutletSend, OutletType};
use pd_ext::pd;
use pd_ext::symbol::Symbol;
use pd_ext_macros::external;
use std::ffi::CString;
use std::fs::File;
use std::io::Read;
use std::rc::Rc;
use std::slice;

struct AtsFile {
    header: ATS_HEADER,
}

external! {

    pub struct AtsDump {
        outlet: Rc<dyn OutletSend>,
    }

    impl ControlExternal for AtsDump {
        fn new(builder: &mut dyn ControlExternalBuilder<Self>) -> Self {
            let outlet = builder.new_message_outlet(OutletType::AnyThing);
            Self { outlet }
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

        fn read_file(&mut self, filename: &Symbol) -> std::io::Result<()> {
            let mut file = File::open(filename.as_ref())?;
            let mut header: std::mem::MaybeUninit<ATS_HEADER> = std::mem::MaybeUninit::uninit();
            unsafe {
                let s = slice::from_raw_parts_mut(&mut header as *mut _ as *mut u8, std::mem::size_of::<ATS_HEADER>());
                file.read_exact(s)?;
                let header = header.assume_init();

                if header.mag != 123f64 {
                    return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "magic number does not match"));
                }
            }
            Ok(())
        }

    }
}
