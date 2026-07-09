mod fuzzy_grep;

mod utils;
pub use utils::*; // contains some of the generally available functions and types

#[allow(clippy::module_inception)]
mod grep;
pub use grep::*;

#[cfg(feature = "definitions")]
mod classify;
#[cfg(feature = "definitions")]
pub use classify::*;

#[cfg(test)]
mod grep_tests;
