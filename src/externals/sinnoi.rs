use pd_ext::builder::SignalProcessorExternalBuilder;
use pd_ext::external::SignalProcessorExternal;

pd_ext_macros::external! {
    #[name = "ats/sinnoi~"]
    pub struct AtsSinNoiExternal {}

    impl SignalProcessorExternal for AtsSinNoiExternal {
        fn new(builder: &mut dyn SignalProcessorExternalBuilder<Self>) -> Self {
            builder.new_signal_outlet();
            Self {}
        }
        fn process(
            &mut self,
            _frames: usize,
            inputs: &[&mut [pd_sys::t_float]],
            outputs: &mut [&mut [pd_sys::t_float]],
        ) {
            for (output, input) in outputs.iter_mut().zip(inputs.iter()) {
                output.copy_from_slice(input);
            }
        }
    }
}
