//! RVZ エンコーダ（ISO→RVZ）。
//!
//! 移植元: rvz-converter web/src/encoder と GC Ripper rvz/src（TS 実装）。
//! zstd 圧縮を使うため `encode` feature が必要（wasm では別途注入する想定）。

use sha1::{Digest, Sha1};

use crate::constants::*;
use crate::crypto::{aes128_cbc_decrypt, sha1_into};
use crate::error::RvzError;
use crate::pack::rvz_pack_chunk;
use crate::read::{ReadAt, WriteAt};

const WII_PARTITION_TABLE_OFFSET: u64 = 0x40000;
const WII_PARTITION_HEADER_SIZE: usize = 0x2c0;
const WII_TICKET_SIZE: usize = 0x2a4;
const WII_PARTITION_MAGIC: u32 = 0x10001;
const WII_PARTITION_TICKET_KEY_ADDRESS: usize = 0x1bf;
const WII_PARTITION_TICKET_TITLE_ID_ADDRESS: usize = 0x1dc;
const WII_PARTITION_DATA_OFFSET_ADDRESS: usize = 0x2b8;
const WII_PARTITION_DATA_SIZE_ADDRESS: usize = 0x2bc;
const WII_FST_OFFSET_ADDRESS: usize = 0x424;
const WII_FST_SIZE_ADDRESS: usize = 0x428;

const ZERO_IV: [u8; 16] = [0u8; 16];
const HEADER_1_SIZE: usize = 0x48;
const HEADER_2_SIZE: usize = 0xdc;
const HEADER_AREA_SIZE: usize = HEADER_1_SIZE + HEADER_2_SIZE;

/// Wii 共通鍵
pub const WII_COMMON_KEY: [u8; 16] = [
	0xeb, 0xe4, 0x2a, 0x22, 0x5e, 0x85, 0x93, 0xe4, 0x48, 0xd9, 0xc5, 0x45, 0x73, 0x81, 0xaa, 0xf7,
];

fn be32(b: &[u8], o: usize) -> u32 {
	u32::from_be_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

fn zstd_compress(data: &[u8], level: i32) -> Result<Vec<u8>, RvzError> {
	zstd::bulk::compress(data, level).map_err(|_| RvzError::Zstd)
}

#[derive(Clone, Copy)]
pub struct PartitionCandidate {
	pub offset: u64,
	pub ptype: u32,
}

#[derive(Clone)]
pub struct ValidPartition {
	pub offset: u64,
	pub ptype: u32,
	pub crypto_key: [u8; 16],
	pub data_offset: u32,
	pub data_start: u64,
	pub data_size: u64,
	pub fst_offset: u64,
	pub fst_size: u64,
}

#[derive(Clone, Copy)]
pub struct CompressRawEntry {
	pub offset: u64,
	pub size: u64,
	pub gi: u32,
	pub ng: u32,
}

#[derive(Clone, Copy, Default)]
pub struct PartitionDataEntry {
	pub fs: u32,
	pub ns: u32,
	pub gi: u32,
	pub ng: u32,
}

#[derive(Clone)]
pub struct CompressPartitionEntry {
	pub key: [u8; 16],
	pub data: [PartitionDataEntry; 2],
}

/// パーティション表を検出する（VolumeWii::GetPartitions 相当）。
pub fn detect_partitions<R: ReadAt>(
	iso_size: u64,
	read: &R,
) -> Result<Vec<PartitionCandidate>, RvzError> {
	let mut candidates = Vec::new();
	if iso_size < WII_PARTITION_TABLE_OFFSET + 0x20 {
		return Ok(candidates);
	}
	let header = read
		.read_at(WII_PARTITION_TABLE_OFFSET, 0x20)
		.map_err(|_| RvzError::BadHeader)?;
	if header.len() < 0x20 {
		return Ok(candidates);
	}
	for g in 0..4 {
		let count = be32(&header, g * 8);
		if count == 0 {
			continue;
		}
		let table_offset = be32(&header, g * 8 + 4) as u64 * 4;
		if table_offset + count as u64 * 8 > iso_size {
			continue;
		}
		let Ok(table) = read.read_at(table_offset, count as usize * 8) else {
			continue;
		};
		for i in 0..count as usize {
			if i * 8 + 8 > table.len() {
				break;
			}
			let offset = be32(&table, i * 8) as u64 * 4;
			let ptype = be32(&table, i * 8 + 4);
			if offset < iso_size {
				candidates.push(PartitionCandidate { offset, ptype });
			}
		}
	}
	candidates.sort_by_key(|c| c.offset);
	Ok(candidates)
}

/// 1 パーティションの検証・タイトルキー導出・FST 読み込み。
pub fn set_up_partition<R: ReadAt>(
	partition: &PartitionCandidate,
	iso_size: u64,
	read: &R,
) -> Result<Option<ValidPartition>, RvzError> {
	let p_off = partition.offset;
	if p_off + WII_PARTITION_HEADER_SIZE as u64 > iso_size {
		return Ok(None);
	}
	let buf = read
		.read_at(p_off, WII_PARTITION_HEADER_SIZE)
		.map_err(|_| RvzError::BadHeader)?;
	if buf.len() < WII_PARTITION_HEADER_SIZE || be32(&buf, 0) != WII_PARTITION_MAGIC {
		return Ok(None);
	}
	let data_offset = be32(&buf, WII_PARTITION_DATA_OFFSET_ADDRESS) as u64 * 4;
	let mut data_size = be32(&buf, WII_PARTITION_DATA_SIZE_ADDRESS) as u64 * 4;
	let data_start = p_off + data_offset;
	if !data_start.is_multiple_of(BLOCK_TOTAL_SIZE as u64) || data_size < BLOCK_TOTAL_SIZE as u64 {
		return Ok(None);
	}
	if data_start + data_size > iso_size {
		data_size = iso_size - data_start;
	}
	if data_size < BLOCK_TOTAL_SIZE as u64 {
		return Ok(None);
	}

	// タイトルキー = AES-128-CBC 復号(共通鍵, IV=titleID, チケット[0x1BF:0x1CF])
	let ticket = &buf[..WII_TICKET_SIZE];
	let mut iv = [0u8; 16];
	iv[0..8].copy_from_slice(
		&ticket[WII_PARTITION_TICKET_TITLE_ID_ADDRESS..WII_PARTITION_TICKET_TITLE_ID_ADDRESS + 8],
	);
	let title_key = aes128_cbc_decrypt(
		&WII_COMMON_KEY,
		&iv,
		&ticket[WII_PARTITION_TICKET_KEY_ADDRESS..WII_PARTITION_TICKET_KEY_ADDRESS + 16],
	);
	let mut crypto_key = [0u8; 16];
	crypto_key.copy_from_slice(&title_key);

	let mut fst_offset = 0u64;
	let mut fst_size = 0u64;
	if let Ok(block0) = read.read_at(data_start, BLOCK_TOTAL_SIZE) {
		if block0.len() == BLOCK_TOTAL_SIZE {
			let data_plain = aes128_cbc_decrypt(
				&crypto_key,
				&block0[0x3d0..0x3e0],
				&block0[BLOCK_HEADER_SIZE..BLOCK_TOTAL_SIZE],
			);
			if data_plain.len() >= 0x42c {
				fst_offset = be32(&data_plain, WII_FST_OFFSET_ADDRESS) as u64 * 4;
				fst_size = be32(&data_plain, WII_FST_SIZE_ADDRESS) as u64 * 4;
			}
		}
	}

	Ok(Some(ValidPartition {
		offset: p_off,
		ptype: partition.ptype,
		crypto_key,
		data_offset: data_offset as u32,
		data_start,
		data_size,
		fst_offset,
		fst_size,
	}))
}

enum Region {
	Raw {
		span_start: u64,
		span_end: u64,
		ng: u32,
	},
	Part {
		partition_idx: usize,
		data_start: u64,
		blocks: u32,
	},
}

struct DataLayout {
	regions: Vec<Region>,
	raw_entries: Vec<CompressRawEntry>,
	partition_entries: Vec<CompressPartitionEntry>,
	group_count: u32,
}

fn build_data_entries(
	iso_size: u64,
	chunk_size: u32,
	valid_partitions: &[ValidPartition],
) -> DataLayout {
	let mut raw_entries = Vec::new();
	let mut partition_entries = Vec::new();
	let mut regions = Vec::new();
	let mut group_index: u32 = 0;

	let push_raw = |offset: u64,
	                size: u64,
	                raw_entries: &mut Vec<CompressRawEntry>,
	                regions: &mut Vec<Region>,
	                group_index: &mut u32| {
		if size == 0 {
			return;
		}
		let mut entry_offset = offset;
		let mut entry_size = size;
		let skip = if offset < DISC_HEADER_SIZE as u64 {
			(DISC_HEADER_SIZE as u64 - offset).min(size)
		} else {
			0
		};
		entry_offset += skip;
		entry_size -= skip;
		if entry_size == 0 {
			return;
		}
		let span_start = align_down(entry_offset, BLOCK_TOTAL_SIZE as u64);
		let span_end = entry_offset + entry_size;
		let ng = span_end
			.saturating_sub(span_start)
			.div_ceil(chunk_size as u64) as u32;
		let gi = *group_index;
		*group_index += ng;
		raw_entries.push(CompressRawEntry {
			offset: entry_offset,
			size: entry_size,
			gi,
			ng,
		});
		regions.push(Region::Raw {
			span_start,
			span_end,
			ng,
		});
	};

	let create_pde = |offset: u64, size: u64, group_index: &mut u32| -> PartitionDataEntry {
		let rounded = align_down(size, BLOCK_TOTAL_SIZE as u64);
		let ng = if rounded == 0 {
			0
		} else {
			rounded.div_ceil(chunk_size as u64) as u32
		};
		let gi = *group_index;
		*group_index += ng;
		PartitionDataEntry {
			fs: (offset / BLOCK_TOTAL_SIZE as u64) as u32,
			ns: (size / BLOCK_TOTAL_SIZE as u64) as u32,
			gi,
			ng,
		}
	};

	let mut last_end: u64 = 0;
	for (pi, p) in valid_partitions.iter().enumerate() {
		if p.offset < last_end {
			continue;
		}
		push_raw(
			last_end,
			p.offset - last_end,
			&mut raw_entries,
			&mut regions,
			&mut group_index,
		);
		push_raw(
			p.offset,
			p.data_offset as u64,
			&mut raw_entries,
			&mut regions,
			&mut group_index,
		);

		let data_start = p.data_start;
		let data_end = data_start + p.data_size;
		let split =
			data_end.min(align_up(p.fst_offset + p.fst_size, GROUP_TOTAL_SIZE as u64) + data_start);
		let e0 = create_pde(data_start, split - data_start, &mut group_index);
		let e1 = create_pde(split, data_end - split, &mut group_index);
		partition_entries.push(CompressPartitionEntry {
			key: p.crypto_key,
			data: [e0, e1],
		});

		for entry in [e0, e1] {
			if entry.ns == 0 {
				continue;
			}
			regions.push(Region::Part {
				partition_idx: pi,
				data_start: entry.fs as u64 * BLOCK_TOTAL_SIZE as u64,
				blocks: entry.ns,
			});
		}
		last_end = (e1.fs as u64 + e1.ns as u64) * BLOCK_TOTAL_SIZE as u64;
	}

	push_raw(
		last_end,
		iso_size - last_end,
		&mut raw_entries,
		&mut regions,
		&mut group_index,
	);
	DataLayout {
		regions,
		raw_entries,
		partition_entries,
		group_count: group_index,
	}
}

/// HashGroup と同一の再計算ハッシュブロック列を返す（64 × 0x400）。
fn hash_group(data_plains: &[Vec<u8>]) -> Vec<Vec<u8>> {
	let zero1k = vec![0u8; 0x400];
	let mut h0s = vec![0u8; BLOCKS_PER_GROUP * H0_ARRAY_SIZE];
	let mut h1 = vec![0u8; BLOCKS_PER_GROUP * 160];
	let mut h2 = vec![0u8; 8 * 20];

	for i in 0..BLOCKS_PER_GROUP {
		let base = i * H0_ARRAY_SIZE;
		match data_plains.get(i) {
			Some(d) => {
				for j in 0..31 {
					sha1_into(d, j * 0x400, 0x400, &mut h0s, base + j * 20);
				}
			}
			// 移植元は範囲外読み出しが 0 になるため ZERO_1K を 31 回ハッシュする（offset 0 で等価）
			None => {
				for j in 0..31 {
					sha1_into(&zero1k, 0, 0x400, &mut h0s, base + j * 20);
				}
			}
		}
	}
	for k in 0..BLOCKS_PER_GROUP / 8 {
		for m in 0..8 {
			sha1_into(
				&h0s,
				(k * 8 + m) * H0_ARRAY_SIZE,
				H0_ARRAY_SIZE,
				&mut h1,
				k * 160 + m * 20,
			);
		}
		sha1_into(&h1, k * 160, 160, &mut h2, k * 20);
	}

	let mut out = vec![vec![0u8; BLOCK_HEADER_SIZE]; BLOCKS_PER_GROUP];
	for i in 0..BLOCKS_PER_GROUP {
		let hb = &mut out[i];
		hb[0..H0_ARRAY_SIZE].copy_from_slice(&h0s[i * H0_ARRAY_SIZE..(i + 1) * H0_ARRAY_SIZE]);
		hb[0x280..0x280 + 160].copy_from_slice(&h1[(i >> 3) * 160..(i >> 3) * 160 + 160]);
		hb[0x340..0x340 + 160].copy_from_slice(&h2);
	}
	out
}

fn collect_hash_exceptions(
	orig: &[u8],
	recomputed: &[u8],
	block_in_chunk: usize,
	exceptions: &mut Vec<(u16, [u8; 20])>,
) {
	let ranges: [(usize, usize); 6] = [
		(0x000, H0_ARRAY_SIZE),
		(H0_ARRAY_SIZE, 20),
		(0x280, 8 * 20),
		(0x320, 32),
		(0x340, 8 * 20),
		(0x3e0, 32),
	];
	for (base, size) in ranges {
		let mut l = 0;
		while l < size {
			let slot = base + l.min(size - 20);
			let diff = (0..20).any(|k| orig[slot + k] != recomputed[slot + k]);
			if diff {
				let mut hash = [0u8; 20];
				hash.copy_from_slice(&orig[slot..slot + 20]);
				exceptions.push(((block_in_chunk * BLOCK_HEADER_SIZE + slot) as u16, hash));
			}
			l += 20;
		}
	}
}

fn build_exception_list(exceptions: &[(u16, [u8; 20])]) -> Vec<u8> {
	let mut bytes = vec![0u8; 2 + exceptions.len() * 22];
	bytes[0..2].copy_from_slice(&(exceptions.len() as u16).to_be_bytes());
	for (i, (offset, hash)) in exceptions.iter().enumerate() {
		let o = 2 + i * 22;
		bytes[o..o + 2].copy_from_slice(&offset.to_be_bytes());
		bytes[o + 2..o + 22].copy_from_slice(hash);
	}
	bytes
}

pub struct CompressInfo {
	pub iso_size: u64,
	pub group_count: u32,
	pub raw_entries: Vec<CompressRawEntry>,
	pub partition_entries: Vec<CompressPartitionEntry>,
}

/// ISO を RVZ へ圧縮する。`on_chunk(offset, stored, packed, compressed)` でチャンクを通知する。
#[allow(clippy::type_complexity)]
pub fn compress_iso<R: ReadAt, F: FnMut(u64, &[u8], usize, bool) -> Result<(), RvzError>>(
	iso_size: u64,
	disc_type: u32,
	chunk_size: u32,
	level: i32,
	read: &R,
	mut on_chunk: F,
) -> Result<CompressInfo, RvzError> {
	let mut valid_partitions: Vec<ValidPartition> = Vec::new();
	if disc_type == 2 {
		for c in detect_partitions(iso_size, read)? {
			if let Some(v) = set_up_partition(&c, iso_size, read)? {
				valid_partitions.push(v);
			}
		}
	}
	let layout = build_data_entries(iso_size, chunk_size, &valid_partitions);

	for region in &layout.regions {
		match region {
			Region::Raw {
				span_start,
				span_end,
				ng,
			} => {
				for i in 0..*ng as u64 {
					let start = span_start + i * chunk_size as u64;
					let end = (*span_end).min(start + chunk_size as u64);
					let len = end - start;
					if len == 0 {
						break;
					}
					let data = read
						.read_at(start, len as usize)
						.map_err(|_| RvzError::BadHeader)?;
					let (main_data, packed_size) = rvz_pack_chunk(&data, start);
					let compressed = zstd_compress(&main_data, level)?;
					let (stored, is_compressed) = if compressed.len() < main_data.len() {
						(compressed, true)
					} else {
						(main_data, false)
					};
					on_chunk(start, &stored, packed_size, is_compressed)?;
				}
			}
			Region::Part {
				partition_idx,
				data_start,
				blocks,
				..
			} => {
				let part = &valid_partitions[*partition_idx];
				compress_partition_region(part, *data_start, *blocks, level, read, &mut on_chunk)?;
			}
		}
	}

	Ok(CompressInfo {
		iso_size,
		group_count: layout.group_count,
		raw_entries: layout.raw_entries,
		partition_entries: layout.partition_entries,
	})
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn compress_partition_region<
	R: ReadAt,
	F: FnMut(u64, &[u8], usize, bool) -> Result<(), RvzError>,
>(
	part: &ValidPartition,
	data_start: u64,
	blocks: u32,
	level: i32,
	read: &R,
	on_chunk: &mut F,
) -> Result<(), RvzError> {
	let blocks_per_chunk = 4u32;
	let mut block_offset = 0u32;
	let mut chunk_index_in_entry = 0u64;

	while block_offset < blocks {
		let blocks_in_window = BLOCKS_PER_GROUP.min((blocks - block_offset) as usize);
		let read_len = blocks_in_window * BLOCK_TOTAL_SIZE;
		let encrypted = read
			.read_at(
				data_start + block_offset as u64 * BLOCK_TOTAL_SIZE as u64,
				read_len,
			)
			.map_err(|_| RvzError::BadHeader)?;
		let chunk_count = blocks_in_window.div_ceil(blocks_per_chunk as usize);

		let mut data_plains: Vec<Vec<u8>> = Vec::with_capacity(blocks_in_window);
		let mut hash_orig: Vec<Vec<u8>> = Vec::with_capacity(blocks_in_window);
		for b in 0..blocks_in_window {
			let blk = &encrypted[b * BLOCK_TOTAL_SIZE..(b + 1) * BLOCK_TOTAL_SIZE];
			let hash_header =
				aes128_cbc_decrypt(&part.crypto_key, &ZERO_IV, &blk[0..BLOCK_HEADER_SIZE]);
			let data_plain = aes128_cbc_decrypt(
				&part.crypto_key,
				&blk[0x3d0..0x3e0],
				&blk[BLOCK_HEADER_SIZE..BLOCK_TOTAL_SIZE],
			);
			data_plains.push(data_plain);
			hash_orig.push(hash_header);
		}
		let recomputed = hash_group(&data_plains);

		for c in 0..chunk_count {
			let first = c * blocks_per_chunk as usize;
			let blocks_in_chunk = (blocks_per_chunk as usize).min(blocks_in_window - first);
			let mut stripped = vec![0u8; blocks_in_chunk * BLOCK_DATA_SIZE];
			for b in 0..blocks_in_chunk {
				stripped[b * BLOCK_DATA_SIZE..(b + 1) * BLOCK_DATA_SIZE]
					.copy_from_slice(&data_plains[first + b]);
			}

			let mut exceptions: Vec<(u16, [u8; 20])> = Vec::new();
			for b in 0..blocks_in_chunk {
				collect_hash_exceptions(
					&hash_orig[first + b],
					&recomputed[first + b],
					b,
					&mut exceptions,
				);
			}
			let exception_list = build_exception_list(&exceptions);

			let chunk_offset =
				chunk_index_in_entry * (blocks_per_chunk as u64 * BLOCK_DATA_SIZE as u64);
			let (main_data, packed_size) = rvz_pack_chunk(&stripped, chunk_offset);
			let mut combined = Vec::with_capacity(exception_list.len() + main_data.len());
			combined.extend_from_slice(&exception_list);
			combined.extend_from_slice(&main_data);
			let compressed = zstd_compress(&combined, level)?;

			let (stored, is_compressed) =
				if compressed.len() < main_data.len() + align4(exception_list.len()) {
					(compressed, true)
				} else {
					let pad = (4 - (exception_list.len() % 4)) % 4;
					let mut s = vec![0u8; exception_list.len() + pad + main_data.len()];
					s[0..exception_list.len()].copy_from_slice(&exception_list);
					s[exception_list.len() + pad..].copy_from_slice(&main_data);
					(s, false)
				};

			on_chunk(
				data_start + block_offset as u64 * BLOCK_TOTAL_SIZE as u64,
				&stored,
				packed_size,
				is_compressed,
			)?;
			chunk_index_in_entry += 1;
		}

		block_offset += blocks_in_window as u32;
	}
	Ok(())
}

#[allow(clippy::too_many_arguments)]
fn build_rvz_headers(
	iso_size: u64,
	wia_size: u64,
	compression_level: u32,
	chunk_size: u32,
	disc_type: u32,
	disc_header: &[u8],
	part_offset: u64,
	part_count: u32,
	part_table: &[u8],
	raw_offset: u64,
	raw_count: u32,
	raw_size: u32,
	group_offset: u64,
	group_size: u32,
	group_count: u32,
) -> Vec<u8> {
	let mut h2 = vec![0u8; HEADER_2_SIZE];
	h2[0x00..0x04].copy_from_slice(&disc_type.to_be_bytes());
	h2[0x04..0x08].copy_from_slice(&5u32.to_be_bytes());
	h2[0x08..0x0c].copy_from_slice(&compression_level.to_be_bytes());
	h2[0x0c..0x10].copy_from_slice(&chunk_size.to_be_bytes());
	let n = disc_header.len().min(0x80);
	h2[0x10..0x10 + n].copy_from_slice(&disc_header[..n]);
	h2[0x90..0x94].copy_from_slice(&part_count.to_be_bytes());
	h2[0x94..0x98].copy_from_slice(&0x30u32.to_be_bytes());
	h2[0x98..0xa0].copy_from_slice(&part_offset.to_be_bytes());
	h2[0xb4..0xb8].copy_from_slice(&raw_count.to_be_bytes());
	h2[0xb8..0xc0].copy_from_slice(&raw_offset.to_be_bytes());
	h2[0xc0..0xc4].copy_from_slice(&raw_size.to_be_bytes());
	h2[0xc4..0xc8].copy_from_slice(&group_count.to_be_bytes());
	h2[0xc8..0xd0].copy_from_slice(&group_offset.to_be_bytes());
	h2[0xd0..0xd4].copy_from_slice(&group_size.to_be_bytes());
	h2[0xd4] = 0;
	let partition_hash = Sha1::digest(part_table);
	h2[0xa0..0xa0 + 20].copy_from_slice(&partition_hash);
	let h2_hash = Sha1::digest(&h2);

	let mut h1 = vec![0u8; HEADER_1_SIZE];
	h1[0..4].copy_from_slice(&[0x52, 0x56, 0x5a, 0x01]);
	h1[0x04..0x08].copy_from_slice(&0x01000000u32.to_be_bytes());
	h1[0x08..0x0c].copy_from_slice(&0x00030000u32.to_be_bytes());
	h1[0x0c..0x10].copy_from_slice(&(HEADER_2_SIZE as u32).to_be_bytes());
	h1[0x10..0x10 + 20].copy_from_slice(&h2_hash);
	h1[0x24..0x2c].copy_from_slice(&iso_size.to_be_bytes());
	h1[0x2c..0x34].copy_from_slice(&wia_size.to_be_bytes());
	let h1_hash = Sha1::digest(&h1[0..0x34]);
	h1[0x34..0x34 + 20].copy_from_slice(&h1_hash);

	let mut out = vec![0u8; HEADER_AREA_SIZE];
	out[0..HEADER_1_SIZE].copy_from_slice(&h1);
	out[HEADER_1_SIZE..].copy_from_slice(&h2);
	out
}

#[allow(clippy::type_complexity)]
fn build_tables(
	group_entries: &[(u32, u32, u32, bool)],
	level: i32,
	raw_entries: &[CompressRawEntry],
	partition_entries: &[CompressPartitionEntry],
) -> Result<(Vec<u8>, Vec<u8>, Vec<u8>), RvzError> {
	let mut part_table = vec![0u8; partition_entries.len() * PART_ENTRY_SIZE];
	for (p, pe) in partition_entries.iter().enumerate() {
		let off = p * PART_ENTRY_SIZE;
		part_table[off..off + 16].copy_from_slice(&pe.key);
		for (k, e) in pe.data.iter().enumerate() {
			let eo = off + 0x10 + k * PART_DATA_ENTRY_SIZE;
			part_table[eo..eo + 4].copy_from_slice(&e.fs.to_be_bytes());
			part_table[eo + 4..eo + 8].copy_from_slice(&e.ns.to_be_bytes());
			part_table[eo + 8..eo + 12].copy_from_slice(&e.gi.to_be_bytes());
			part_table[eo + 12..eo + 16].copy_from_slice(&e.ng.to_be_bytes());
		}
	}

	let mut raw_table = vec![0u8; raw_entries.len() * RAW_ENTRY_SIZE];
	for (i, e) in raw_entries.iter().enumerate() {
		let o = i * RAW_ENTRY_SIZE;
		raw_table[o..o + 8].copy_from_slice(&e.offset.to_be_bytes());
		raw_table[o + 8..o + 16].copy_from_slice(&e.size.to_be_bytes());
		raw_table[o + 16..o + 20].copy_from_slice(&e.gi.to_be_bytes());
		raw_table[o + 20..o + 24].copy_from_slice(&e.ng.to_be_bytes());
	}

	let mut group_table = vec![0u8; group_entries.len() * GROUP_ENTRY_SIZE];
	for (i, (off4, stored_size, packed, compressed)) in group_entries.iter().enumerate() {
		let o = i * GROUP_ENTRY_SIZE;
		group_table[o..o + 4].copy_from_slice(&off4.to_be_bytes());
		let size = stored_size | if *compressed { 0x80000000 } else { 0 };
		group_table[o + 4..o + 8].copy_from_slice(&size.to_be_bytes());
		group_table[o + 8..o + 12].copy_from_slice(&packed.to_be_bytes());
	}

	Ok((
		part_table,
		zstd_compress(&raw_table, level)?,
		zstd_compress(&group_table, level)?,
	))
}

/// ISO を読み、RVZ を `write` へ組み立てる。戻り値は出力 RVZ のサイズ。
pub fn encode_iso_to_rvz<R: ReadAt, W: WriteAt>(
	iso_size: u64,
	level: i32,
	read: &R,
	write: &mut W,
) -> Result<u64, RvzError> {
	let mut header = vec![0u8; DISC_HEADER_SIZE];
	let h = read
		.read_at(0, DISC_HEADER_SIZE)
		.map_err(|_| RvzError::BadHeader)?;
	let n = h.len().min(DISC_HEADER_SIZE);
	header[..n].copy_from_slice(&h[..n]);
	let disc_type = if n >= 0x1c && be32(&header, 0x18) == WII_MAGIC {
		2
	} else {
		1
	};

	let chunk_size = CHUNK_SIZE as u32;
	let mut pos: u64 = HEADER_AREA_SIZE as u64;
	// ヘッダ領域をゼロで確保
	write
		.write_at(0, &vec![0u8; HEADER_AREA_SIZE])
		.map_err(RvzError::Io)?;

	let mut group_entries: Vec<(u32, u32, u32, bool)> = Vec::new();
	let info = compress_iso(
		iso_size,
		disc_type,
		chunk_size,
		level,
		read,
		|_offset, stored, packed, compressed| {
			write.write_at(pos, stored).map_err(RvzError::Io)?;
			let off4 = (pos / 4) as u32;
			pos += stored.len() as u64;
			let pad = (4 - (pos % 4)) % 4;
			if pad > 0 {
				write
					.write_at(pos, &[0u8; 4][..pad as usize])
					.map_err(RvzError::Io)?;
				pos += pad;
			}
			group_entries.push((off4, stored.len() as u32, packed as u32, compressed));
			Ok(())
		},
	)?;

	let (part_table, raw_table, group_table) = build_tables(
		&group_entries,
		level,
		&info.raw_entries,
		&info.partition_entries,
	)?;

	let part_offset = if part_table.is_empty() { 0 } else { pos };
	if !part_table.is_empty() {
		write.write_at(pos, &part_table).map_err(RvzError::Io)?;
		pos += part_table.len() as u64;
		let pad = (4 - (pos % 4)) % 4;
		if pad > 0 {
			write
				.write_at(pos, &[0u8; 4][..pad as usize])
				.map_err(RvzError::Io)?;
			pos += pad;
		}
	}
	let raw_offset = pos;
	write.write_at(pos, &raw_table).map_err(RvzError::Io)?;
	pos += raw_table.len() as u64;
	let pad = (4 - (pos % 4)) % 4;
	if pad > 0 {
		write
			.write_at(pos, &[0u8; 4][..pad as usize])
			.map_err(RvzError::Io)?;
		pos += pad;
	}
	let group_offset = pos;
	write.write_at(pos, &group_table).map_err(RvzError::Io)?;
	pos += group_table.len() as u64;
	let wia_size = pos;

	let headers = build_rvz_headers(
		iso_size,
		wia_size,
		level as u32,
		chunk_size,
		disc_type,
		&header,
		part_offset,
		(part_table.len() / PART_ENTRY_SIZE) as u32,
		&part_table,
		raw_offset,
		info.raw_entries.len() as u32,
		raw_table.len() as u32,
		group_offset,
		group_table.len() as u32,
		group_entries.len() as u32,
	);
	write.write_at(0, &headers).map_err(RvzError::Io)?;

	Ok(wia_size)
}
