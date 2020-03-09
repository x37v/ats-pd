mod cache;
mod data;
mod externals;

use std::convert::TryFrom;

extern "C" {
    fn atsdataexternal_setup();
    fn atssinnoiexternal_tilde_setup();
}

#[no_mangle]
pub unsafe extern "C" fn ats_setup() {
    atsdataexternal_setup();
    atssinnoiexternal_tilde_setup();

    let help =
        pd_ext::symbol::Symbol::try_from("ats-data-help").expect("failed to create help sym");
    pd_sys::class_sethelpsymbol(
        crate::externals::data::ATSDATAEXTERNAL_CLASS.unwrap(),
        help.inner(),
    );
}
