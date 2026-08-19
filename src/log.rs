/// Central logging facade: with the `defmt` feature the real defmt macros are
/// used, otherwise no-op stubs that discard their arguments (the arguments are
/// not name-resolved in that case, so e.g. `defmt::Display2Format` stays valid
/// without the feature).
#[cfg(feature = "defmt")]
pub(crate) use defmt::{debug, error, info};

#[cfg(not(feature = "defmt"))]
pub(crate) use noop::*;

#[cfg(not(feature = "defmt"))]
mod noop {
    macro_rules! debug {
        ($($arg:tt)*) => {};
    }
    macro_rules! info {
        ($($arg:tt)*) => {};
    }
    macro_rules! error {
        ($($arg:tt)*) => {};
    }
    pub(crate) use {debug, error, info};
}
