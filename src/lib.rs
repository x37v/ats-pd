mod cache;
mod data;
mod externals;

extern "C" {
    fn atsdataexternal_setup();
    fn atssinnoiexternal_tilde_setup();
}

#[no_mangle]
pub unsafe extern "C" fn ats_setup() {
    atsdataexternal_setup();
    atssinnoiexternal_tilde_setup();
}
