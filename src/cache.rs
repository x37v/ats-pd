use crate::data::AtsData;
use pd_ext::symbol::Symbol;
use std::collections::HashMap;
use std::ffi::CString;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::sync::{Arc, Weak};

static COUNT: AtomicUsize = AtomicUsize::new(0);

//mutex should be fine because all PD methods should be accessing from the same thread
lazy_static::lazy_static! {
    static ref HASH: Mutex<HashMap<Symbol, Weak<AtsData>>> = {
        Mutex::new(HashMap::new())
    };
}

//insert, returning the key
pub fn insert(data: Arc<AtsData>) -> Symbol {
    let c = COUNT.fetch_add(1, Ordering::Relaxed);
    let s: String = data
        .source
        .chars()
        .map(|x| match x {
            '/' => '-',
            c @ 'A'..='Z' => c,
            c @ 'a'..='z' => c,
            c @ '0'..='9' => c,
            _ => '_',
        })
        .collect();
    let k = format!("{}-{}", c, s);
    let k = Symbol::from(CString::new(k).unwrap());

    (*HASH).lock().unwrap().insert(k, Arc::downgrade(&data));
    k
}

pub fn get(key: Symbol) -> Option<Arc<AtsData>> {
    let mut out = None;
    let mut h = (*HASH).lock().unwrap();
    if let Some(v) = h.get(&key) {
        out = v.upgrade();
        //cleanup if it is a miss
        if out.is_none() {
            h.remove(&key);
        }
    }
    out
}
