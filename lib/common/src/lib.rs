#![doc = include_str!("../README.md")]
// `Time` / `TimePlugin` live in the `time` module; the lint reads the
// re-exported names as repeating the module name. Mirrors `spark-core`.
#![allow(clippy::module_name_repetitions)]

mod time;

pub use time::{Time, TimePlugin};
