use pd_ext::builder::ControlExternalBuilder;
use pd_ext::external::ControlExternal;
use pd_ext::outlet::{OutletSend, OutletType};
use pd_ext::pd;
use pd_ext::symbol::Symbol;
use pd_ext_macros::external;
use std::ffi::CString;
use std::rc::Rc;

external! {
    pub struct AtsDump {
        outlet: Rc<dyn OutletSend>,
        open_sym: Symbol
    }

    impl ControlExternal for AtsDump {
        fn new(builder: &mut dyn ControlExternalBuilder<Self>) -> Self {
            let outlet = builder.new_message_outlet(OutletType::AnyThing);
            let open_sym = CString::new("open").unwrap().into();
            Self { outlet, open_sym }
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
            self.post(format!("got filename {}", filename));
        }

    }
}
