mod client;
mod codec;

#[cfg(test)]
pub use codec::MAX_RECORD_BYTES_FOR_TEST;
pub use codec::{CodecError, read_record, write_record};
