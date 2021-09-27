#![allow(non_upper_case_globals)]
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]
#![allow(dead_code)]
#![allow(deref_nullptr)]
#![allow(clippy::all, clippy::pedantic)]

include!(concat!(
    env!("XASKPASS_BUILD_HEADER_DIR"),
    "/xkbcommon-names.rs"
));
