#![allow(dead_code)]
pub type Key = usize;

#[inline]
pub unsafe fn create(_dtor: Option<unsafe extern "C" fn(*mut u8)>) -> Key {
    panic!("should not be used on this target");
}

#[inline]
pub unsafe fn set(_key: Key, _value: *mut u8) {
    panic!("should not be used on this target");
}

#[inline]
pub unsafe fn get(_key: Key) -> *mut u8 {
    panic!("should not be used on this target");
}

#[inline]
pub unsafe fn destroy(_key: Key) {
    panic!("should not be used on this target");
}
