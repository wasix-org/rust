//@run-rustfix
//@aux-build:proc_macros.rs:proc-macro
#![allow(clippy::eq_op, clippy::needless_raw_string_hashes, clippy::no_effect, unused)]
#![warn(clippy::string_lit_chars_any)]

#[macro_use]
extern crate proc_macros;

struct NotStringLit;

impl NotStringLit {
    fn chars(&self) -> impl Iterator<Item = char> {
        "c".chars()
    }
}

fn main() {
    let c = 'c';
    "\\.+*?()|[]{}^$#&-~".chars().any(|x| x == c);
    r#"\.+*?()|[]{}^$#&-~"#.chars().any(|x| x == c);
    "\\.+*?()|[]{}^$#&-~".chars().any(|x| c == x);
    r#"\.+*?()|[]{}^$#&-~"#.chars().any(|x| c == x);
    #[rustfmt::skip]
    "\\.+*?()|[]{}^$#&-~".chars().any(|x| { x == c });
    // Do not lint
    NotStringLit.chars().any(|x| x == c);
    "\\.+*?()|[]{}^$#&-~".chars().any(|x| {
        let c = 'c';
        x == c
    });
    "\\.+*?()|[]{}^$#&-~".chars().any(|x| {
        1;
        x == c
    });
    "\\.+*?()|[]{}^$#&-~".chars().any(|x| x == x);
    "\\.+*?()|[]{}^$#&-~".chars().any(|_x| c == c);
    matches!(
        c,
        '\\' | '.' | '+' | '*' | '(' | ')' | '|' | '[' | ']' | '{' | '}' | '^' | '$' | '#' | '&' | '-' | '~'
    );
    external! {
        let c = 'c';
        "\\.+*?()|[]{}^$#&-~".chars().any(|x| x == c);
    }
    with_span! {
        span
        let c = 'c';
        "\\.+*?()|[]{}^$#&-~".chars().any(|x| x == c);
    }
}
