//! RVZ packing 展開（LFG パディングレコードの展開）。
//!
//! 移植元: rvz-converter web/src/decoder/unpack.ts

use crate::error::RvzError;
use crate::lfg::Lfg;

/// packing 済みデータ `bytes` を展開し、`expected` バイトを生成する。
/// `data_offset` は出力先の絶対オフセット（パディング位相に使用）。
pub fn unpack(
	bytes: &[u8],
	expected: usize,
	data_offset: u64,
	packed_size: usize,
) -> Result<Vec<u8>, RvzError> {
	if bytes.len() != packed_size {
		return Err(RvzError::PackingSizeMismatch);
	}
	let mut output = vec![0u8; expected];
	let mut read = 0usize;
	let mut written = 0usize;

	while read < bytes.len() && written < expected {
		if read + 4 > bytes.len() {
			return Err(RvzError::TruncatedRecord);
		}
		let raw = u32::from_be_bytes([
			bytes[read],
			bytes[read + 1],
			bytes[read + 2],
			bytes[read + 3],
		]);
		read += 4;
		let junk = (raw & 0x80000000) != 0;
		let size = (raw & 0x7fffffff) as usize;

		if junk {
			if read + 68 > bytes.len() {
				return Err(RvzError::TruncatedSeed);
			}
			if written + size > expected {
				return Err(RvzError::PaddingOverflow);
			}
			let mut lfg = Lfg::new(&bytes[read..read + 68]);
			read += 68;
			lfg.forward_bytes(((data_offset + written as u64) % 0x8000) as usize);
			output[written..written + size].copy_from_slice(&lfg.bytes(size));
			written += size;
		} else {
			if read + size > bytes.len() || written + size > expected {
				return Err(RvzError::BadRecord);
			}
			output[written..written + size].copy_from_slice(&bytes[read..read + size]);
			read += size;
			written += size;
		}
	}

	if read != bytes.len() {
		return Err(RvzError::UnconsumedInput);
	}
	if written != expected {
		return Err(RvzError::SizeMismatch);
	}
	Ok(output)
}
