//@run-rustfix
//@aux-build:proc_macros.rs:proc-macro
#![feature(custom_inner_attributes)]
#![allow(unused)]
#![warn(clippy::four_forward_slashes)]
#![no_main]
#![rustfmt::skip]

#[macro_use]
extern crate proc_macros;

//// whoops
fn a() {}

//// whoops
#[allow(dead_code)]
fn b() {}

//// whoops
//// two borked comments!
#[track_caller]
fn c() {}

fn d() {}

#[test]
//// between attributes
#[allow(dead_code)]
fn g() {}

    //// not very start of contents
fn h() {}

fn i() {
    //// don't lint me bozo
    todo!()
}

external! {
    //// don't lint me bozo
    fn e() {}
}

with_span! {
    span
    //// don't lint me bozo
    fn f() {}
}
