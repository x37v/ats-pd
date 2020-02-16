mod data;

extern "C" {
    fn atsdata_setup();
}

#[no_mangle]
pub unsafe extern "C" fn ats_setup() {
    atsdata_setup();
}
