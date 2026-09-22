//! RVZ packing（LFG パディング検出・junk レコード化）。
//! Dolphin RVZPack 相当。チャンクを 0x8000 境界で走査し、パディング領域を junk レコード化する。

use crate::constants::BLOCK_TOTAL_SIZE;
use crate::lfg::{seed_bytes, u32be, LaggedFibonacciGenerator, LFG_SEED_BYTES};

/// チャンクを packing し、`(mainData, packedSize)` を返す。
/// パディングが見つからなければ `(data のコピー, 0)`。
pub fn rvz_pack_chunk(data: &[u8], data_offset: u64) -> (Vec<u8>, usize) {
	// (start, end, seed)
	let mut junk_info: Vec<(usize, usize, [u32; 17])> = Vec::new();
	let mut position: usize = 0;
	let mut scan_offset: u64 = data_offset;

	while position < data.len() {
		let mut zeroes = 0usize;
		while position + zeroes < data.len() && data[position + zeroes] == 0 {
			zeroes += 1;
		}
		position += zeroes;
		scan_offset += zeroes as u64;

		// 絶対オフセットのままビット演算でアラインすると 2GB 超で負値化するため u64 で計算する
		let aligned_next =
			scan_offset + (BLOCK_TOTAL_SIZE as u64 - (scan_offset % BLOCK_TOTAL_SIZE as u64));
		let bytes_to_read = ((aligned_next - scan_offset) as usize).min(data.len() - position);
		let data_offset_mod = (scan_offset % BLOCK_TOTAL_SIZE as u64) as usize;

		let mut lfg = LaggedFibonacciGenerator::new();
		let (count, seed) = lfg.get_seed(
			&data[position..position + bytes_to_read],
			bytes_to_read,
			data_offset_mod,
		);
		if count > 0 {
			if let Some(seed) = seed {
				junk_info.push((position, position + count, seed));
			}
		}

		position += bytes_to_read;
		scan_offset += bytes_to_read as u64;
	}

	let end_offset = data.len();
	let mut parts: Vec<u8> = Vec::new();
	let mut packed_size = 0usize;
	let mut current_offset = 0usize;
	let mut first_loop_iteration = true;

	while current_offset < end_offset {
		let mut junk: Option<(usize, usize, [u32; 17])> = None;
		let mut next_junk_start = end_offset;
		let mut next_junk_end = end_offset;
		if end_offset - current_offset > LFG_SEED_BYTES {
			for &candidate in &junk_info {
				if candidate.1 > current_offset + LFG_SEED_BYTES {
					if candidate.0 + LFG_SEED_BYTES < end_offset {
						junk = Some(candidate);
						next_junk_start = current_offset.max(candidate.0);
						next_junk_end = end_offset.min(candidate.1);
					}
					break;
				}
			}
		}

		if first_loop_iteration {
			if next_junk_start == end_offset {
				return (data.to_vec(), 0);
			}
			first_loop_iteration = false;
		}

		let non_junk_bytes = next_junk_start - current_offset;
		if non_junk_bytes > 0 {
			parts.extend_from_slice(&u32be(non_junk_bytes as u32));
			parts.extend_from_slice(&data[current_offset..current_offset + non_junk_bytes]);
			current_offset += non_junk_bytes;
			packed_size += 4 + non_junk_bytes;
		}

		let junk_bytes = next_junk_end - current_offset;
		if junk_bytes > 0 {
			let Some(junk) = junk else {
				break; // 通常は到達しない（安全弁）
			};
			parts.extend_from_slice(&u32be((junk_bytes as u32) | 0x80000000));
			parts.extend_from_slice(&seed_bytes(&junk.2));
			current_offset += junk_bytes;
			packed_size += 4 + LFG_SEED_BYTES;
		}
	}

	(parts, packed_size)
}
