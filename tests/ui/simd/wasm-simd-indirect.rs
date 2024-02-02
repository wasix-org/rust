// build-pass

#[cfg(any(target_family = "wasm", target_arch = "wasix32", target_arch = "wasix64"))]
fn main() {
    unsafe {
        a::api_with_simd_feature();
    }
}

#[cfg(any(target_family = "wasm", target_arch = "wasix32", target_arch = "wasix64"))]
mod a {
    use std::arch::wasm::*;

    #[target_feature(enable = "simd128")]
    pub unsafe fn api_with_simd_feature() {
        crate::b::api_takes_v128(u64x2(0, 1));
    }
}

#[cfg(any(target_family = "wasm", target_arch = "wasix32", target_arch = "wasix64"))]
mod b {
    use std::arch::wasm::*;

    #[inline(never)]
    pub fn api_takes_v128(a: v128) -> v128 {
        a
    }
}

#[cfg(not(any(target_family = "wasm", target_arch = "wasix32", target_arch = "wasix64")))]
fn main() {}
