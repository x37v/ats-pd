mod data;

extern "C" {
    fn atsdataexternal_setup();
}

#[no_mangle]
pub unsafe extern "C" fn ats_setup() {
    atsdataexternal_setup();
}
