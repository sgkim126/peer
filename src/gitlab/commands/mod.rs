mod api;
mod error;

#[expect(unused_imports)]
pub use api::Collection;
pub use error::CommandError;

#[cfg(all(test, unix))]
mod tests;
