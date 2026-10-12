mod api;
mod error;

pub use error::CommandError;

#[cfg(all(test, unix))]
mod tests;
