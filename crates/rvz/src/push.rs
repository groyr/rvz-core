//! RVZ→ISO の **push 型** デコーダ。
//!
//! 非同期 I/O（ブラウザの fetch / OPFS）と両立させるため、wasm 側から読みに行かず
//! 「次に読む位置」を [`PushDecoder::request`] で要求し、JS が読んだバイトを
//! [`PushDecoder::feed`] で渡す。出力は [`PushDecoder::take_output`] で `(offset, data)` を取り出す。
//!
//! 展開ロジックは `decoder.rs`（pull 型）と同一。

use std::collections::VecDeque;

use crate::constants::*;
use crate::decoder::{
	finalize_group, parse_header_min, parse_parts, part_table_info, table_bytes, zstd_decompress,
	GroupEntry, Partition, PartitionDataEntry, RawEntry, RvzHeader,
};
use crate::error::RvzError;
use crate::unpack::unpack;

const HEADER_1_SIZE: usize = 0x48;

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

/// 次に読むべき位置と長さ。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReadRequest {
	pub offset: u64,
	pub len: usize,
}

enum Stage {
	Header1,
	Header2,
	PartTable,
	RawTable,
	GroupTable,
	Regions,
	Done,
}

#[derive(Clone)]
enum Region {
	Raw(RawEntry),
	Part(Partition, PartitionDataEntry),
}

struct PartState {
	key: [u8; 16],
	part_chunk_size: u64,
	data_size: u64,
	disc_base: u64,
	gi: u32,
	ng: u64,
	i: u64,
	block_index: usize,
	group_start_block: usize,
	group_blocks: Vec<Option<Vec<u8>>>,
	group_ex: Vec<Vec<(usize, [u8; 20])>>,
	h0s: Vec<u8>,
	h1: Vec<u8>,
	h2: Vec<u8>,
	zero1k: Vec<u8>,
}

impl PartState {
	fn new(p: &Partition, pe: PartitionDataEntry, chunk_size: u32) -> Self {
		let part_chunk_size =
			(chunk_size as u64 * BLOCK_DATA_SIZE as u64) / BLOCK_TOTAL_SIZE as u64;
		Self {
			key: p.key,
			part_chunk_size,
			data_size: pe.ns as u64 * BLOCK_DATA_SIZE as u64,
			disc_base: pe.fs as u64 * BLOCK_TOTAL_SIZE as u64,
			gi: pe.gi,
			ng: pe.ng as u64,
			i: 0,
			block_index: 0,
			group_start_block: 0,
			group_blocks: vec![None; BLOCKS_PER_GROUP],
			group_ex: vec![Vec::new(); BLOCKS_PER_GROUP],
			h0s: vec![0u8; BLOCKS_PER_GROUP * H0_ARRAY_SIZE],
			h1: vec![0u8; 8 * 160],
			h2: vec![0u8; 8 * 20],
			zero1k: vec![0u8; 0x400],
		}
	}
}

/// push 型デコーダ本体。
pub struct PushDecoder {
	file_size: u64,
	header: Option<RvzHeader>,
	part_req: Option<(u64, u32, u32)>,
	raw_entries: Vec<RawEntry>,
	groups: Vec<GroupEntry>,
	regions: Vec<Region>,
	region_idx: usize,
	raw_i: u64,
	part: Option<PartState>,
	stage: Stage,
	request: Option<ReadRequest>,
	outputs: VecDeque<(u64, Vec<u8>)>,
	error: Option<String>,
}

impl PushDecoder {
	pub fn new(file_size: u64) -> Self {
		Self {
			file_size,
			header: None,
			part_req: None,
			raw_entries: Vec::new(),
			groups: Vec::new(),
			regions: Vec::new(),
			region_idx: 0,
			raw_i: 0,
			part: None,
			stage: Stage::Header1,
			request: Some(ReadRequest {
				offset: 0,
				len: HEADER_1_SIZE,
			}),
			outputs: VecDeque::new(),
			error: None,
		}
	}

	/// 次に読むべき位置（None なら完了）。
	pub fn request(&self) -> Option<ReadRequest> {
		self.request
	}
	pub fn is_done(&self) -> bool {
		matches!(self.stage, Stage::Done)
	}
	pub fn output_size(&self) -> u64 {
		self.header.as_ref().map(|h| h.iso_size).unwrap_or(0)
	}
	/// ヘッダー内のディスクヘッダ（dhead, 0x80 バイト）。
	pub fn dhead(&self) -> Option<&[u8]> {
		self.header.as_ref().map(|h| h.dhead.as_slice())
	}
	pub fn has_output(&self) -> bool {
		!self.outputs.is_empty()
	}
	pub fn take_output(&mut self) -> Option<(u64, Vec<u8>)> {
		self.outputs.pop_front()
	}
	pub fn error(&self) -> Option<&str> {
		self.error.as_deref()
	}

	/// 直前の [`Self::request`] に対するバイト列を渡す。
	pub fn feed(&mut self, bytes: &[u8]) -> Result<(), RvzError> {
		let r = self.feed_inner(bytes);
		if let Err(e) = &r {
			self.error = Some(e.to_string());
		}
		r
	}

	fn feed_inner(&mut self, bytes: &[u8]) -> Result<(), RvzError> {
		match self.stage {
			Stage::Header1 => {
				if bytes.len() < HEADER_1_SIZE || &bytes[0..3] != b"RVZ" {
					return Err(RvzError::NotRvz);
				}
				let header2_size = be32(bytes, 0x0c) as usize;
				if header2_size < 0xdc || self.file_size < (HEADER_1_SIZE + header2_size) as u64 {
					return Err(RvzError::BadHeader);
				}
				self.request = Some(ReadRequest {
					offset: 0,
					len: HEADER_1_SIZE + header2_size,
				});
				self.stage = Stage::Header2;
			}
			Stage::Header2 => {
				let header = parse_header_min(bytes, self.file_size)?;
				if header.compression != 5 {
					return Err(RvzError::UnsupportedCompression);
				}
				let (po, esz) = part_table_info(bytes);
				self.part_req = Some((po, header.part_count, esz));
				self.header = Some(header);
				if self.header.as_ref().unwrap().part_count > 0 {
					let (o, cnt, esz) = self.part_req.unwrap();
					self.request = Some(ReadRequest {
						offset: o,
						len: cnt as usize * esz as usize,
					});
					self.stage = Stage::PartTable;
				} else {
					let h = self.header.as_ref().unwrap();
					self.request = Some(ReadRequest {
						offset: h.raw_offset,
						len: h.raw_size as usize,
					});
					self.stage = Stage::RawTable;
				}
			}
			Stage::PartTable => {
				let (_, cnt, esz) = self.part_req.unwrap();
				let parts = parse_parts(bytes, cnt, esz)?;
				self.header.as_mut().unwrap().parts = parts;
				let h = self.header.as_ref().unwrap();
				self.request = Some(ReadRequest {
					offset: h.raw_offset,
					len: h.raw_size as usize,
				});
				self.stage = Stage::RawTable;
			}
			Stage::RawTable => {
				let count = self.header.as_ref().unwrap().raw_count as usize;
				let table = table_bytes(bytes.to_vec(), count * RAW_ENTRY_SIZE)?;
				self.raw_entries = (0..count)
					.map(|i| {
						let o = i * RAW_ENTRY_SIZE;
						RawEntry {
							offset: be64(&table, o),
							size: be64(&table, o + 8),
							group: be32(&table, o + 16),
							groups: be32(&table, o + 20),
						}
					})
					.collect();
				let h = self.header.as_ref().unwrap();
				self.request = Some(ReadRequest {
					offset: h.group_offset,
					len: h.group_size as usize,
				});
				self.stage = Stage::GroupTable;
			}
			Stage::GroupTable => {
				let count = self.header.as_ref().unwrap().group_count as usize;
				let table = table_bytes(bytes.to_vec(), count * GROUP_ENTRY_SIZE)?;
				self.groups = (0..count)
					.map(|i| {
						let o = i * GROUP_ENTRY_SIZE;
						GroupEntry {
							offset4: be32(&table, o),
							size: be32(&table, o + 4),
							packed_size: be32(&table, o + 8),
						}
					})
					.collect();
				self.build_regions();
				self.stage = Stage::Regions;
				self.region_idx = 0;
				self.raw_i = 0;
				self.part = None;
				self.issue_region_request()?;
			}
			Stage::Regions => {
				if self.part.is_some() {
					self.process_part_group(bytes)?;
				} else {
					self.process_raw_group(bytes)?;
				}
			}
			Stage::Done => {}
		}
		Ok(())
	}

	fn build_regions(&mut self) {
		let mut regions = Vec::new();
		for part in &self.header.as_ref().unwrap().parts {
			for pe in &part.data {
				if pe.ns == 0 {
					continue;
				}
				regions.push(Region::Part(part.clone(), *pe));
			}
		}
		for raw in &self.raw_entries {
			regions.push(Region::Raw(*raw));
		}
		regions.sort_by_key(|r| match r {
			Region::Part(_, pe) => pe.fs as u64 * BLOCK_TOTAL_SIZE as u64,
			Region::Raw(e) => e.offset - (e.offset % BLOCK_TOTAL_SIZE as u64),
		});
		self.regions = regions;
	}

	fn next_region(&mut self) -> Result<(), RvzError> {
		self.region_idx += 1;
		self.raw_i = 0;
		self.part = None;
		self.issue_region_request()
	}

	fn issue_region_request(&mut self) -> Result<(), RvzError> {
		if self.region_idx >= self.regions.len() {
			self.stage = Stage::Done;
			self.request = None;
			return Ok(());
		}
		match self.regions[self.region_idx].clone() {
			Region::Raw(e) => {
				if self.raw_i >= e.groups as u64 {
					return self.next_region();
				}
				let g = self.groups[e.group as usize + self.raw_i as usize];
				let stored = (g.size & 0x7fffffff) as usize;
				self.request = Some(ReadRequest {
					offset: g.offset4 as u64 * 4,
					len: stored,
				});
				Ok(())
			}
			Region::Part(p, pe) => {
				let chunk_size = self.header.as_ref().unwrap().chunk_size;
				self.part = Some(PartState::new(&p, pe, chunk_size));
				self.issue_part_request()
			}
		}
	}

	fn issue_part_request(&mut self) -> Result<(), RvzError> {
		let Some(ps) = self.part.as_ref() else {
			return self.next_region();
		};
		if ps.i >= ps.ng {
			return self.next_region();
		}
		let (gi, i) = (ps.gi, ps.i);
		let g = self.groups[gi as usize + i as usize];
		let stored = (g.size & 0x7fffffff) as usize;
		self.request = Some(ReadRequest {
			offset: g.offset4 as u64 * 4,
			len: stored,
		});
		Ok(())
	}

	fn process_raw_group(&mut self, bytes: &[u8]) -> Result<(), RvzError> {
		let Region::Raw(e) = self.regions[self.region_idx].clone() else {
			return Err(RvzError::BadRecord);
		};
		let chunk_size = self.header.as_ref().unwrap().chunk_size as u64;
		let g = self.groups[e.group as usize + self.raw_i as usize];
		let skipped = e.offset % BLOCK_TOTAL_SIZE as u64;
		let logical_size = e.size + skipped;
		let group_logical_offset = self.raw_i * chunk_size;
		let logical_offset = e.offset - skipped + group_logical_offset;
		let expected = chunk_size.min(logical_size - group_logical_offset) as usize;
		let stored_size = (g.size & 0x7fffffff) as usize;

		let chunk = if stored_size != 0 {
			let decompressed = if (g.size & 0x80000000) != 0 {
				zstd_decompress(bytes)?
			} else {
				bytes.to_vec()
			};
			if g.packed_size != 0 {
				unpack(
					&decompressed,
					expected,
					logical_offset,
					g.packed_size as usize,
				)?
			} else {
				if decompressed.len() != expected {
					return Err(RvzError::SizeMismatch);
				}
				decompressed
			}
		} else {
			vec![0u8; expected]
		};

		self.outputs.push_back((logical_offset, chunk));
		self.raw_i += 1;
		self.issue_region_request()
	}

	fn process_part_group(&mut self, bytes: &[u8]) -> Result<(), RvzError> {
		let (gi, i, part_chunk_size, data_size, disc_base) = {
			let ps = self.part.as_ref().unwrap();
			(ps.gi, ps.i, ps.part_chunk_size, ps.data_size, ps.disc_base)
		};
		let g = self.groups[gi as usize + i as usize];
		let expected = part_chunk_size.min(data_size - i * part_chunk_size) as usize;
		let stored_size = (g.size & 0x7fffffff) as usize;
		let mut exceptions: Vec<(usize, usize, [u8; 20])> = Vec::new();
		let hash_stripped: Vec<u8>;

		if stored_size > 0 {
			let compressed_flag = (g.size & 0x80000000) != 0;
			let decompressed = if compressed_flag {
				zstd_decompress(bytes)?
			} else {
				bytes.to_vec()
			};
			if decompressed.len() < 2 {
				return Err(RvzError::BadRecord);
			}
			let count = u16::from_be_bytes([decompressed[0], decompressed[1]]) as usize;
			let list_size = if compressed_flag {
				2 + count * 22
			} else {
				(2 + count * 22 + 3) & !3
			};
			let packed_size = g.packed_size as usize;
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
			let block_index = self.part.as_ref().unwrap().block_index;
			for e in 0..count {
				let o = 2 + e * 22;
				let off = u16::from_be_bytes([decompressed[o], decompressed[o + 1]]) as usize;
				let bic = off / BLOCK_HEADER_SIZE;
				let in_block = off % BLOCK_HEADER_SIZE;
				if bic >= blocks_in_chunk || in_block + 20 > BLOCK_HEADER_SIZE {
					return Err(RvzError::BadRecord);
				}
				let mut hh = [0u8; 20];
				hh.copy_from_slice(&decompressed[o + 2..o + 22]);
				exceptions.push((block_index + bic, in_block, hh));
			}
		} else {
			hash_stripped = vec![0u8; expected];
		}

		let blocks_in_chunk = hash_stripped.len() / BLOCK_DATA_SIZE;
		let ps = self.part.as_mut().unwrap();
		for b in 0..blocks_in_chunk {
			let gidx = (ps.block_index + b) % BLOCKS_PER_GROUP;
			ps.group_blocks[gidx] =
				Some(hash_stripped[b * BLOCK_DATA_SIZE..(b + 1) * BLOCK_DATA_SIZE].to_vec());
		}
		for (abs, inb, hh) in exceptions {
			ps.group_ex[abs % BLOCKS_PER_GROUP].push((inb, hh));
		}
		ps.block_index += blocks_in_chunk;
		ps.i += 1;

		if ps.block_index.is_multiple_of(BLOCKS_PER_GROUP) || ps.i == ps.ng {
			let blocks_in_this_group = BLOCKS_PER_GROUP.min(ps.block_index - ps.group_start_block);
			if blocks_in_this_group > 0 {
				let output = finalize_group(
					&ps.key,
					&ps.group_blocks,
					&ps.group_ex,
					blocks_in_this_group,
					&mut ps.h0s,
					&mut ps.h1,
					&mut ps.h2,
					&ps.zero1k,
				)?;
				let offset = disc_base + ps.group_start_block as u64 * BLOCK_TOTAL_SIZE as u64;
				self.outputs.push_back((offset, output));
			}
			for b in ps.group_blocks.iter_mut() {
				*b = None;
			}
			for e in ps.group_ex.iter_mut() {
				e.clear();
			}
			ps.group_start_block = ps.block_index;
		}

		self.issue_part_request()
	}
}
