#![feature(diagnostic_namespace)]

#[diagnostic::on_unimplemented(
    message = "Message",
    note = "Note",
    label = "label"
)]
pub trait Foo {}
