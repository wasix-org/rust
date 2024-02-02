// check-pass

#[cfg(any(target_family = "wasm", target_arch = "wasix32", target_arch = "wasix64", doc))]
#[target_feature(enable = "simd128")]
pub fn foo() {}
