//! Nintendo GameCube ディスクのスクランブル解除（descramble）。
//!
//! 出典: friidump (Arep, GPLv2+) の unscrambler を基にした CleanRip
//! `source/disc_scramble.c` → gc-live-disc-server `descramble.py` と同じアルゴリズムを
//! Rust へ移植したもの（PC/Pi 側の重複実装をここへ一本化する）。
//!
//! GameCube のスクランブル seed は disc ごとに異なるため、生フレーム先頭セクタの
//! EDC を使って seed を探索し、16 セクタ周期でキャッシュする。

mod edc_table;

use std::path::{Path, PathBuf};

use edc_table::EDC_TABLE;

/// 生フレーム 1 セクタのサイズ（ユーザデータ + ヘッダ/EDC）
pub const RAW_SECTOR_SIZE: usize = 2064;
/// ユーザデータ 1 セクタのサイズ
pub const USER_SECTOR_SIZE: usize = 2048;
/// スクランブル seed の周期（セクタ数）
pub const SECTORS_PER_BLOCK: usize = 16;
/// 生フレーム 1 ブロック = 16 セクタ
pub const RAW_BLOCK_SIZE: usize = RAW_SECTOR_SIZE * SECTORS_PER_BLOCK; // 33024
/// 解除後 1 ブロック = 16 セクタ
pub const BLOCK_SIZE: usize = USER_SECTOR_SIZE * SECTORS_PER_BLOCK; // 32768
/// EDC がカバーする長さ（先頭 4 バイトを除く）
pub const EDC_LENGTH: usize = RAW_SECTOR_SIZE - 4; // 2060

/// seed 探索の既定上限（これ未満を探索）
pub const DEFAULT_MAX_SEED_SEARCH: u16 = 0x7FFF;

/// 解除時のエラー
#[derive(Debug)]
pub enum DescrambleError {
	RawTooShort,
	SeedNotFound { block: u32 },
	EdcMismatch { block: u32, sector: usize },
}

impl std::fmt::Display for DescrambleError {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		match self {
			DescrambleError::RawTooShort => write!(f, "生フレームが短すぎます"),
			DescrambleError::SeedNotFound { block } => {
				write!(f, "block={} の seed を特定できません", block)
			}
			DescrambleError::EdcMismatch { block, sector } => {
				write!(f, "block={} sector={} の EDC 不一致", block, sector)
			}
		}
	}
}

impl std::error::Error for DescrambleError {}

/// EDC を 1 バイトずつ更新する。
pub fn edc_calc(mut edc: u32, data: &[u8]) -> u32 {
	for &b in data {
		let idx = (((edc >> 24) ^ b as u32) & 0xFF) as usize;
		edc = EDC_TABLE[idx] ^ edc.wrapping_shl(8);
	}
	edc
}

/// seed から 2048 バイトのストリーム暗号を生成する。
pub fn lfsr_stream(seed: u16) -> [u8; USER_SECTOR_SIZE] {
	let mut lfsr = (seed & 0x7FFF) as u32;
	let mut out = [0u8; USER_SECTOR_SIZE];
	for o in out.iter_mut() {
		let mut byte = 0u8;
		for _ in 0..8 {
			let bit = (lfsr >> 14) & 1;
			lfsr = ((lfsr << 1) | (bit ^ ((lfsr >> 10) & 1))) & 0x7FFF;
			byte = (byte << 1) | bit as u8;
		}
		*o = byte;
	}
	out
}

#[inline]
fn xor_in_place(data: &mut [u8], key: &[u8]) {
	for (d, k) in data.iter_mut().zip(key.iter()) {
		*d ^= *k;
	}
}

/// 生フレーム先頭セクタの EDC で seed を判定する。
pub fn test_seed(raw_sector: &[u8], seed: u16) -> bool {
	let mut tmp = raw_sector[..RAW_SECTOR_SIZE].to_vec();
	let cipher = lfsr_stream(seed);
	xor_in_place(&mut tmp[12..EDC_LENGTH], &cipher);
	let calc = edc_calc(0, &tmp[..EDC_LENGTH]);
	let stored = u32::from_be_bytes([
		tmp[EDC_LENGTH],
		tmp[EDC_LENGTH + 1],
		tmp[EDC_LENGTH + 2],
		tmp[EDC_LENGTH + 3],
	]);
	calc == stored
}

/// 16 フェーズ分の seed をキャッシュしながらブロックを解除する。
pub struct Descrambler {
	seeds: [Option<u16>; SECTORS_PER_BLOCK],
	cipher: [Option<[u8; USER_SECTOR_SIZE]>; SECTORS_PER_BLOCK],
	cache_path: Option<PathBuf>,
	max_seed_search: u16,
}

impl Descrambler {
	/// 任意の seed キャッシュ JSON を読み込んで生成する。
	pub fn new(cache_path: Option<PathBuf>) -> Self {
		let mut d = Self {
			seeds: std::array::from_fn(|_| None),
			cipher: std::array::from_fn(|_| None),
			cache_path,
			max_seed_search: DEFAULT_MAX_SEED_SEARCH,
		};
		if let Some(p) = d.cache_path.clone() {
			d.load(&p);
		}
		d
	}

	/// seed 探索の上限を変更する（テスト用）。
	pub fn set_max_seed_search(&mut self, n: u16) {
		self.max_seed_search = n;
	}

	/// テスト用: seed を直接登録する。
	pub fn set_seed(&mut self, phase: usize, seed: u16) {
		let p = phase % SECTORS_PER_BLOCK;
		self.seeds[p] = Some(seed & 0x7FFF);
		self.cipher[p] = None;
	}

	/// phase の seed（未確定なら None）。
	pub fn seed(&self, phase: usize) -> Option<u16> {
		self.seeds[phase % SECTORS_PER_BLOCK]
	}

	fn cipher_for(&mut self, phase: usize) -> [u8; USER_SECTOR_SIZE] {
		let p = phase % SECTORS_PER_BLOCK;
		if let Some(c) = self.cipher[p] {
			return c;
		}
		let seed = self.seeds[p].expect("phase の seed が未確定です");
		let c = lfsr_stream(seed);
		self.cipher[p] = Some(c);
		c
	}

	/// 生ブロック先頭セクタから seed を探索する。
	pub fn find_seed(&mut self, raw_block: &[u8], phase: usize) -> Option<u16> {
		let first = &raw_block[..RAW_SECTOR_SIZE];
		for candidate in 0..self.max_seed_search as u32 {
			let c = candidate as u16;
			if test_seed(first, c) {
				self.set_seed(phase, c);
				self.save();
				return Some(c);
			}
		}
		None
	}

	/// 生フレーム 33024B を 32768B のユーザデータに解除する。
	pub fn unscramble_block(
		&mut self,
		sector_no: u32,
		raw_block: &[u8],
		verify: bool,
	) -> Result<Vec<u8>, DescrambleError> {
		if raw_block.len() < RAW_BLOCK_SIZE {
			return Err(DescrambleError::RawTooShort);
		}
		let block_no = sector_no / SECTORS_PER_BLOCK as u32;
		let phase = (block_no as usize) % SECTORS_PER_BLOCK;

		if self.seeds[phase].is_none() && self.find_seed(raw_block, phase).is_none() {
			return Err(DescrambleError::SeedNotFound { block: block_no });
		}
		let cipher = self.cipher_for(phase);

		let mut out = vec![0u8; BLOCK_SIZE];
		for k in 0..SECTORS_PER_BLOCK {
			let base = k * RAW_SECTOR_SIZE;
			let mut tmp = raw_block[base..base + RAW_SECTOR_SIZE].to_vec();
			xor_in_place(&mut tmp[12..EDC_LENGTH], &cipher);
			if verify {
				let calc = edc_calc(0, &tmp[..EDC_LENGTH]);
				let stored = u32::from_be_bytes([
					tmp[EDC_LENGTH],
					tmp[EDC_LENGTH + 1],
					tmp[EDC_LENGTH + 2],
					tmp[EDC_LENGTH + 3],
				]);
				if calc != stored {
					return Err(DescrambleError::EdcMismatch {
						block: block_no,
						sector: k,
					});
				}
			}
			// Nintendo 方式: tmp[6:2054] の 2048B がユーザデータ
			out[k * USER_SECTOR_SIZE..(k + 1) * USER_SECTOR_SIZE]
				.copy_from_slice(&tmp[6..6 + USER_SECTOR_SIZE]);
		}
		Ok(out)
	}

	fn load(&mut self, path: &Path) {
		let Ok(s) = std::fs::read_to_string(path) else {
			return;
		};
		let Ok(map) = serde_json::from_str::<std::collections::BTreeMap<String, u16>>(&s) else {
			return;
		};
		for (k, v) in map {
			if let Ok(p) = k.parse::<usize>() {
				if p < SECTORS_PER_BLOCK {
					self.seeds[p] = Some(v);
				}
			}
		}
	}

	/// seed キャッシュを JSON へ保存する。
	pub fn save(&self) {
		let Some(path) = &self.cache_path else {
			return;
		};
		let map: std::collections::BTreeMap<String, u16> = self
			.seeds
			.iter()
			.enumerate()
			.filter_map(|(i, s)| s.map(|v| (i.to_string(), v)))
			.collect();
		let Ok(json) = serde_json::to_string(&map) else {
			return;
		};
		let tmp = path.with_extension("json.tmp");
		if std::fs::write(&tmp, json).is_ok() {
			let _ = std::fs::rename(&tmp, path);
		}
	}
}

/// 既知の seed で「正当なスクランブル済みブロック」を生成する（テスト・ツール用）。
pub fn make_test_block(seed: u16, payload: &[u8], lba: u32) -> Vec<u8> {
	assert_eq!(payload.len(), USER_SECTOR_SIZE, "payload は 2048 バイト");
	let cipher = lfsr_stream(seed);
	let mut block = vec![0u8; RAW_BLOCK_SIZE];
	for k in 0..SECTORS_PER_BLOCK {
		let mut tmp = vec![0u8; RAW_SECTOR_SIZE];
		let sn = lba + k as u32 + 0x30000;
		tmp[1] = (sn >> 16) as u8;
		tmp[2] = (sn >> 8) as u8;
		tmp[3] = sn as u8;
		tmp[6..6 + USER_SECTOR_SIZE].copy_from_slice(payload);
		let edc = edc_calc(0, &tmp[..EDC_LENGTH]);
		tmp[EDC_LENGTH..EDC_LENGTH + 4].copy_from_slice(&edc.to_be_bytes());

		let base = k * RAW_SECTOR_SIZE;
		block[base..base + 12].copy_from_slice(&tmp[..12]);
		let mut mid = tmp[12..EDC_LENGTH].to_vec();
		xor_in_place(&mut mid, &cipher);
		block[base + 12..base + EDC_LENGTH].copy_from_slice(&mid);
		block[base + EDC_LENGTH..base + RAW_SECTOR_SIZE]
			.copy_from_slice(&tmp[EDC_LENGTH..RAW_SECTOR_SIZE]);
	}
	block
}
