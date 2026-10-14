pub mod api;
mod error;

pub use api::Collection;
pub use error::CommandError;

#[cfg(all(test, unix))]
mod tests;
