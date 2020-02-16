mod cache;
mod data;
mod externals;

extern "C" {
    fn atsdataexternal_setup();
}

#[no_mangle]
pub unsafe extern "C" fn ats_setup() {
    atsdataexternal_setup();
}
