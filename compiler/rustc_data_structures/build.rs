fn main() {
    println!("cargo:rustc-check-cfg=cfg(target_arch, values(\"wasix32\",\"wasix64\"))");
}