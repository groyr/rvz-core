//! RVZ→ISO デコーダ。
//!
//! 移植元: rvz-converter web/src/decoder（TS 実装）。
//! - ヘッダー／管理テーブルの解析
//! - raw 領域の展開（zstd + LFG unpack）
//! - パーティション領域の展開（zstd + unpack + ハッシュ再構築 + AES-CBC 再暗号化）

use std::io::Read as _;

use crate::constants::*;
use crate::crypto::{aes128_cbc_encrypt, sha1_into};
use crate::error::RvzError;
use crate::read::ReadAt;
use crate::unpack::unpack;

const HEADER_1_SIZE: usize = 0x48;
const HEADER_2_SIZE: usize = 0xdc;
const ZERO_IV: [u8; 16] = [0u8; 16];

#[derive(Clone, Copy, Default)]
pub struct PartitionDataEntry {
	pub fs: u32,
	pub ns: u32,
	pub gi: u32,
	pub ng: u32,
}

#[derive(Clone)]
pub struct Partition {
	pub key: [u8; 16],
	pub data: [PartitionDataEntry; 2],
}

#[derive(Clone, Copy)]
pub struct RawEntry {
	pub offset: u64,
	pub size: u64,
	pub group: u32,
	pub groups: u32,
}

#[derive(Clone, Copy)]
pub struct GroupEntry {
	pub offset4: u32,
	pub size: u32,
	pub packed_size: u32,
}

pub struct RvzHeader {
	pub iso_size: u64,
	pub compression: u32,
	pub chunk_size: u32,
	pub raw_count: u32,
	pub raw_offset: u64,
	pub raw_size: u32,
	pub group_count: u32,
	pub group_offset: u64,
	pub group_size: u32,
	pub part_count: u32,
	pub parts: Vec<Partition>,
	pub dhead: Vec<u8>,
}

fn be32(b: &[u8], o: usize) -> u32 {
	u32::from_be_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
fn be64(b: &[u8], o: usize) -> u64 {
	u64::from_be_bytes([
		b[o],
		b[o + 1],
		b[o + 2],
		b[o + 3],
		b[o + 4],
		b[o + 5],
		b[o + 6],
		b[o + 7],
	])
}

pub(crate) fn zstd_decompress(input: &[u8]) -> Result<Vec<u8>, RvzError> {
	let mut dec = ruzstd::StreamingDecoder::new(input).map_err(|_| RvzError::Zstd)?;
	let mut out = Vec::new();
	dec.read_to_end(&mut out).map_err(|_| RvzError::Zstd)?;
	Ok(out)
}

/// RVZ ヘッダーを読み、出力 ISO サイズを返す（小さな先頭読みのみ）。
pub fn read_output_size<R: ReadAt>(read: &R) -> Result<u64, RvzError> {
	let bytes = read.read_at(0, 0x30).map_err(|_| RvzError::BadHeader)?;
	if bytes.len() < 0x30 || &bytes[0..3] != b"RVZ" {
		return Err(RvzError::NotRvz);
	}
	Ok(be64(&bytes, 0x24))
}

/// ヘッダー領域のバイト列を解析する（パーティション表は含めない）。
pub fn parse_header_min(bytes: &[u8], file_size: u64) -> Result<RvzHeader, RvzError> {
	if bytes.len() < HEADER_1_SIZE || &bytes[0..3] != b"RVZ" {
		return Err(RvzError::NotRvz);
	}
	let header2_size = be32(bytes, 0x0c) as usize;
	if header2_size < HEADER_2_SIZE || file_size < HEADER_1_SIZE as u64 + header2_size as u64 {
		return Err(RvzError::BadHeader);
	}
	if be64(bytes, 0x2c) != file_size {
		return Err(RvzError::FileSizeMismatch);
	}
	let h = &bytes[HEADER_1_SIZE..];
	let mut dhead = vec![0u8; DISC_HEADER_SIZE];
	let dhead_start = HEADER_1_SIZE + 0x10;
	if dhead_start + DISC_HEADER_SIZE <= bytes.len() {
		dhead.copy_from_slice(&bytes[dhead_start..dhead_start + DISC_HEADER_SIZE]);
	}
	Ok(RvzHeader {
		iso_size: be64(bytes, 0x24),
		compression: be32(h, 0x04),
		chunk_size: be32(h, 0x0c),
		raw_count: be32(h, 0xb4),
		raw_offset: be64(h, 0xb8),
		raw_size: be32(h, 0xc0),
		group_count: be32(h, 0xc4),
		group_offset: be64(h, 0xc8),
		group_size: be32(h, 0xd0),
		part_count: be32(h, 0x90),
		parts: Vec::new(),
		dhead,
	})
}

/// ヘッダーからパーティション表の位置とエントリサイズを取り出す。
pub fn part_table_info(header_bytes: &[u8]) -> (u64, u32) {
	let h = &header_bytes[HEADER_1_SIZE..];
	(be64(h, 0x98), be32(h, 0x94))
}

/// パーティション表のバイト列を解析する。
pub fn parse_parts(
	table: &[u8],
	part_count: u32,
	part_entry_size: u32,
) -> Result<Vec<Partition>, RvzError> {
	if part_entry_size < PART_ENTRY_SIZE as u32 {
		return Err(RvzError::BadPartitionTable);
	}
	let mut parts = Vec::with_capacity(part_count as usize);
	for p in 0..part_count as usize {
		let e = &table[p * part_entry_size as usize..];
		let mut key = [0u8; 16];
		key.copy_from_slice(&e[0..16]);
		let mut data = [PartitionDataEntry::default(); 2];
		for (k, d) in data.iter_mut().enumerate() {
			let off = 0x10 + k * PART_DATA_ENTRY_SIZE;
			*d = PartitionDataEntry {
				fs: be32(e, off),
				ns: be32(e, off + 4),
				gi: be32(e, off + 8),
				ng: be32(e, off + 12),
			};
		}
		parts.push(Partition { key, data });
	}
	Ok(parts)
}

/// 管理テーブルのバイト列を展開する（長さが期待値と違う場合のみ zstd 展開）。
pub(crate) fn table_bytes(table: Vec<u8>, expected: usize) -> Result<Vec<u8>, RvzError> {
	if table.len() != expected {
		let out = zstd_decompress(&table)?;
		if out.len() != expected {
			return Err(RvzError::SizeMismatch);
		}
		return Ok(out);
	}
	Ok(table)
}

fn read_header<R: ReadAt>(file_size: u64, read: &R) -> Result<RvzHeader, RvzError> {
	let part1 = read
		.read_at(0, HEADER_1_SIZE)
		.map_err(|_| RvzError::BadHeader)?;
	if part1.len() < HEADER_1_SIZE || &part1[0..3] != b"RVZ" {
		return Err(RvzError::NotRvz);
	}
	let header2_size = be32(&part1, 0x0c);
	if header2_size < HEADER_2_SIZE as u32 || file_size < HEADER_1_SIZE as u64 + header2_size as u64
	{
		return Err(RvzError::BadHeader);
	}
	let bytes = read
		.read_at(0, HEADER_1_SIZE + header2_size as usize)
		.map_err(|_| RvzError::BadHeader)?;
	let mut header = parse_header_min(&bytes, file_size)?;
	if header.part_count > 0 {
		let (part_offset, part_entry_size) = part_table_info(&bytes);
		let table = read
			.read_at(
				part_offset,
				header.part_count as usize * part_entry_size as usize,
			)
			.map_err(|_| RvzError::BadPartitionTable)?;
		header.parts = parse_parts(&table, header.part_count, part_entry_size)?;
	}
	Ok(header)
}

fn decompress_table<R: ReadAt>(
	read: &R,
	offset: u64,
	size: u32,
	expected: usize,
) -> Result<Vec<u8>, RvzError> {
	let table = read
		.read_at(offset, size as usize)
		.map_err(|_| RvzError::BadHeader)?;
	if table.len() != expected {
		let out = zstd_decompress(&table)?;
		if out.len() != expected {
			return Err(RvzError::SizeMismatch);
		}
		return Ok(out);
	}
	Ok(table)
}

fn decompress_raw<R: ReadAt, W: FnMut(u64, &[u8])>(
	raw: &RawEntry,
	header: &RvzHeader,
	groups: &[GroupEntry],
	read: &R,
	write: &mut W,
) -> Result<(), RvzError> {
	let skipped = raw.offset % BLOCK_TOTAL_SIZE as u64;
	let logical_size = raw.size + skipped;
	for i in 0..raw.groups as u64 {
		let group_logical_offset = i * header.chunk_size as u64;
		let logical_offset = raw.offset - skipped + group_logical_offset;
		let expected = (header.chunk_size as u64).min(logical_size - group_logical_offset) as usize;
		if expected == 0 || expected > header.chunk_size as usize {
			return Err(RvzError::SizeMismatch);
		}
		let group = groups
			.get(raw.group as usize + i as usize)
			.ok_or(RvzError::MissingGroup)?;
		let stored_size = (group.size & 0x7fffffff) as usize;
		let chunk: Vec<u8>;
		if stored_size != 0 {
			let compressed = read
				.read_at(group.offset4 as u64 * 4, stored_size)
				.map_err(|_| RvzError::BadHeader)?;
			let decompressed = if (group.size & 0x80000000) != 0 {
				zstd_decompress(&compressed)?
			} else {
				compressed
			};
			if group.packed_size != 0 {
				chunk = unpack(
					&decompressed,
					expected,
					logical_offset,
					group.packed_size as usize,
				)?;
			} else {
				if decompressed.len() != expected {
					return Err(RvzError::SizeMismatch);
				}
				chunk = decompressed;
			}
		} else {
			chunk = vec![0u8; expected];
		}
		write(logical_offset, &chunk);
	}
	Ok(())
}

#[allow(clippy::too_many_arguments, clippy::needless_range_loop)]
pub(crate) fn finalize_group(
	key: &[u8; 16],
	blocks: &[Option<Vec<u8>>],
	ex: &[Vec<(usize, [u8; 20])>],
	n: usize,
	h0s: &mut [u8],
	h1: &mut [u8],
	h2: &mut [u8],
	zero1k: &[u8],
) -> Result<Vec<u8>, RvzError> {
	for j in 0..BLOCKS_PER_GROUP {
		let base = j * H0_ARRAY_SIZE;
		match &blocks[j] {
			Some(bd) => {
				for s in 0..31 {
					sha1_into(bd, s * 0x400, 0x400, h0s, base + s * 20);
				}
			}
			None => {
				for s in 0..31 {
					sha1_into(zero1k, 0, 0x400, h0s, base + s * 20);
				}
			}
		}
	}
	for k in 0..8 {
		let h1base = k * 160;
		for m in 0..8 {
			sha1_into(
				h0s,
				(k * 8 + m) * H0_ARRAY_SIZE,
				H0_ARRAY_SIZE,
				h1,
				h1base + m * 20,
			);
		}
		sha1_into(h1, h1base, 160, h2, k * 20);
	}

	let mut output = vec![0u8; n * BLOCK_TOTAL_SIZE];
	for j in 0..n {
		let mut hash_block = vec![0u8; BLOCK_HEADER_SIZE];
		hash_block[0..H0_ARRAY_SIZE]
			.copy_from_slice(&h0s[j * H0_ARRAY_SIZE..(j + 1) * H0_ARRAY_SIZE]);
		hash_block[0x280..0x280 + 160].copy_from_slice(&h1[(j >> 3) * 160..(j >> 3) * 160 + 160]);
		hash_block[0x340..0x340 + 160].copy_from_slice(&h2[0..160]);
		for (in_block, hash) in &ex[j] {
			hash_block[*in_block..*in_block + 20].copy_from_slice(hash);
		}
		let hash_cipher = aes128_cbc_encrypt(key, &ZERO_IV, &hash_block);
		let iv = &hash_cipher[0x3d0..0x3e0];
		let data = blocks[j].as_ref().ok_or(RvzError::MissingGroup)?;
		let data_cipher = aes128_cbc_encrypt(key, iv, data);
		output[j * BLOCK_TOTAL_SIZE..j * BLOCK_TOTAL_SIZE + BLOCK_HEADER_SIZE]
			.copy_from_slice(&hash_cipher);
		output[j * BLOCK_TOTAL_SIZE + BLOCK_HEADER_SIZE..(j + 1) * BLOCK_TOTAL_SIZE]
			.copy_from_slice(&data_cipher);
	}
	Ok(output)
}

#[allow(clippy::too_many_arguments)]
fn decompress_partition<R: ReadAt, W: FnMut(u64, &[u8])>(
	part: &Partition,
	pe: &PartitionDataEntry,
	groups: &[GroupEntry],
	header: &RvzHeader,
	read: &R,
	write: &mut W,
) -> Result<(), RvzError> {
	if pe.ns == 0 {
		return Ok(());
	}
	let part_chunk_size =
		(header.chunk_size as u64 * BLOCK_DATA_SIZE as u64) / BLOCK_TOTAL_SIZE as u64;
	let data_size = pe.ns as u64 * BLOCK_DATA_SIZE as u64;
	let disc_base = pe.fs as u64 * BLOCK_TOTAL_SIZE as u64;

	let mut group_blocks: Vec<Option<Vec<u8>>> = vec![None; BLOCKS_PER_GROUP];
	let mut group_ex: Vec<Vec<(usize, [u8; 20])>> = vec![Vec::new(); BLOCKS_PER_GROUP];
	let mut group_h0s = vec![0u8; BLOCKS_PER_GROUP * H0_ARRAY_SIZE];
	let mut group_h1 = vec![0u8; 8 * 160];
	let mut group_h2 = vec![0u8; 8 * 20];
	let zero1k = vec![0u8; 0x400];

	let mut block_index: usize = 0;
	let mut group_start_block: usize = 0;

	for i in 0..pe.ng as u64 {
		let group = groups
			.get(pe.gi as usize + i as usize)
			.ok_or(RvzError::MissingGroup)?;
		let expected = part_chunk_size.min(data_size - i * part_chunk_size) as usize;
		if expected == 0 || expected > part_chunk_size as usize {
			return Err(RvzError::SizeMismatch);
		}
		let stored_size = (group.size & 0x7fffffff) as usize;
		let mut exceptions: Vec<(usize, usize, [u8; 20])> = Vec::new();
		let hash_stripped: Vec<u8>;

		if stored_size > 0 {
			let compressed = read
				.read_at(group.offset4 as u64 * 4, stored_size)
				.map_err(|_| RvzError::BadHeader)?;
			let compressed_flag = (group.size & 0x80000000) != 0;
			let decompressed = if compressed_flag {
				zstd_decompress(&compressed)?
			} else {
				compressed
			};
			if decompressed.len() < 2 {
				return Err(RvzError::BadRecord);
			}
			let count = u16::from_be_bytes([decompressed[0], decompressed[1]]) as usize;
			// 圧縮チャンクは例外リストにパディングなし。無圧縮は 4 バイト境界へパディング
			let list_size = if compressed_flag {
				2 + count * 22
			} else {
				(2 + count * 22 + 3) & !3
			};
			let packed_size = group.packed_size as usize;
			if list_size + packed_size > decompressed.len() {
				return Err(RvzError::BadRecord);
			}
			let packed = &decompressed[list_size..list_size + packed_size];
			if packed_size > 0 {
				hash_stripped = unpack(packed, expected, i * part_chunk_size, packed_size)?;
			} else {
				hash_stripped = decompressed[list_size..list_size + expected].to_vec();
				if hash_stripped.len() != expected {
					return Err(RvzError::SizeMismatch);
				}
			}
			let blocks_in_chunk = expected / BLOCK_DATA_SIZE;
			for e in 0..count {
				let o = 2 + e * 22;
				let off = u16::from_be_bytes([decompressed[o], decompressed[o + 1]]) as usize;
				let block_in_chunk = off / BLOCK_HEADER_SIZE;
				let in_block = off % BLOCK_HEADER_SIZE;
				if block_in_chunk >= blocks_in_chunk || in_block + 20 > BLOCK_HEADER_SIZE {
					return Err(RvzError::BadRecord);
				}
				let mut h = [0u8; 20];
				h.copy_from_slice(&decompressed[o + 2..o + 22]);
				exceptions.push((block_index + block_in_chunk, in_block, h));
			}
		} else {
			hash_stripped = vec![0u8; expected];
		}

		let blocks_in_chunk = hash_stripped.len() / BLOCK_DATA_SIZE;
		for b in 0..blocks_in_chunk {
			let gi = (block_index + b) % BLOCKS_PER_GROUP;
			group_blocks[gi] =
				Some(hash_stripped[b * BLOCK_DATA_SIZE..(b + 1) * BLOCK_DATA_SIZE].to_vec());
		}
		for (abs_block, in_block, hash) in exceptions {
			group_ex[abs_block % BLOCKS_PER_GROUP].push((in_block, hash));
		}
		block_index += blocks_in_chunk;

		if block_index.is_multiple_of(BLOCKS_PER_GROUP) || i == pe.ng as u64 - 1 {
			let blocks_in_this_group = BLOCKS_PER_GROUP.min(block_index - group_start_block);
			if blocks_in_this_group > 0 {
				let output = finalize_group(
					&part.key,
					&group_blocks,
					&group_ex,
					blocks_in_this_group,
					&mut group_h0s,
					&mut group_h1,
					&mut group_h2,
					&zero1k,
				)?;
				let offset = disc_base + group_start_block as u64 * BLOCK_TOTAL_SIZE as u64;
				write(offset, &output);
			}
			// 移植元同様、グループ確定後にバッファをクリアする（端数グループで前回値を使わない）
			for b in group_blocks.iter_mut() {
				*b = None;
			}
			for e in group_ex.iter_mut() {
				e.clear();
			}
			group_start_block = block_index;
		}
	}
	Ok(())
}

/// RVZ を展開して ISO を生成する。`write` はオフセットごとに出力を書き込む。
/// 戻り値は ISO の総サイズ。
pub fn decompress_rvz<R: ReadAt, W: FnMut(u64, &[u8])>(
	file_size: u64,
	read: &R,
	mut write: W,
) -> Result<u64, RvzError> {
	let header = read_header(file_size, read)?;
	if header.compression != 5 {
		return Err(RvzError::UnsupportedCompression);
	}
	let raw_bytes = decompress_table(
		read,
		header.raw_offset,
		header.raw_size,
		header.raw_count as usize * RAW_ENTRY_SIZE,
	)?;
	let group_bytes = decompress_table(
		read,
		header.group_offset,
		header.group_size,
		header.group_count as usize * GROUP_ENTRY_SIZE,
	)?;

	let mut raw_entries = Vec::with_capacity(header.raw_count as usize);
	for i in 0..header.raw_count as usize {
		let o = i * RAW_ENTRY_SIZE;
		raw_entries.push(RawEntry {
			offset: be64(&raw_bytes, o),
			size: be64(&raw_bytes, o + 8),
			group: be32(&raw_bytes, o + 16),
			groups: be32(&raw_bytes, o + 20),
		});
	}
	let mut groups = Vec::with_capacity(header.group_count as usize);
	for i in 0..header.group_count as usize {
		let o = i * GROUP_ENTRY_SIZE;
		groups.push(GroupEntry {
			offset4: be32(&group_bytes, o),
			size: be32(&group_bytes, o + 4),
			packed_size: be32(&group_bytes, o + 8),
		});
	}

	enum Region<'a> {
		Part(&'a Partition, &'a PartitionDataEntry, u64),
		Raw(&'a RawEntry, u64),
	}
	let mut regions: Vec<Region> = Vec::new();
	for part in &header.parts {
		for pe in &part.data {
			if pe.ns == 0 {
				continue;
			}
			regions.push(Region::Part(
				part,
				pe,
				pe.fs as u64 * BLOCK_TOTAL_SIZE as u64,
			));
		}
	}
	for raw in &raw_entries {
		let start = raw.offset - (raw.offset % BLOCK_TOTAL_SIZE as u64);
		regions.push(Region::Raw(raw, start));
	}
	regions.sort_by_key(|r| match r {
		Region::Part(_, _, s) => *s,
		Region::Raw(_, s) => *s,
	});

	for region in &regions {
		match region {
			Region::Part(part, pe, _) => {
				decompress_partition(part, pe, &groups, &header, read, &mut write)?;
			}
			Region::Raw(raw, _) => {
				decompress_raw(raw, &header, &groups, read, &mut write)?;
			}
		}
	}

	Ok(header.iso_size)
}
