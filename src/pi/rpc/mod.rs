mod client;
mod codec;

pub use client::{RpcClient, RpcError};
#[cfg(test)]
pub use codec::MAX_RECORD_BYTES_FOR_TEST;
pub use codec::{CodecError, read_record, write_record};
