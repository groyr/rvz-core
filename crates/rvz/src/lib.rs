//! rvz — RVZ コンテナ（Wii ディスクイメージの Zstandard 圧縮コンテナ）の Rust 実装。
//!
//! 移植元: rvz-converter web/src（ブラウザ TS 実装）と GC Ripper rvz/src（Node TS 実装）。
//! 両者に重複していた実装をここへ一本化する（S2 構想）。
//!
//! 現状は packing 層（LFG パディングの pack/unpack）までを移植済み。
//! コンテナ全体（encoder/decoder）は段階的に移植する。

pub mod constants;
pub mod crypto;
pub mod decoder;
#[cfg(feature = "encode")]
pub mod encoder;
pub mod error;
pub mod lfg;
pub mod pack;
pub mod push;
pub mod read;
pub mod unpack;

pub use decoder::{decompress_rvz, read_output_size};
#[cfg(feature = "encode")]
pub use encoder::{compress_iso, encode_iso_to_rvz};
pub use error::RvzError;
pub use lfg::{seed_bytes, u32be, LaggedFibonacciGenerator, Lfg, LFG_SEED_BYTES};
pub use pack::rvz_pack_chunk;
pub use push::{PushDecoder, ReadRequest};
pub use read::{ReadAt, WriteAt};
pub use unpack::unpack;
