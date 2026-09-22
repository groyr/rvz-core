//! LFG（Lagged Fibonacci Generator）。
//! Wii パディング領域の擬似乱数生成（Dolphin と同一アルゴリズム）。
//!
//! - 展開側 [`Lfg`]: RVZ packing のパディング展開用（シードのバイト列からストリーム生成）
//! - 圧縮側 [`LaggedFibonacciGenerator`]: LFG 検出・シード導出用

// 添字に i-17 / i+modv 等の算術を使う数値ループが多く、範囲 for の方が可読性が高いため許可する
#![allow(clippy::needless_range_loop)]

use crate::constants::swap32;

const LFG_K: usize = 521;
const LFG_J: usize = 32;
const LFG_SEED_SIZE: usize = 17;
/// シードのバイト長（17 ワード × 4）
pub const LFG_SEED_BYTES: usize = LFG_SEED_SIZE * 4;

// ---------- 展開側: RVZ packing のパディング展開 ----------

/// シードバイト列（68 バイト）から LFG を初期化し、前方へ進めてバイト列を生成する。
pub struct Lfg {
	buffer: [u32; LFG_K],
	position: usize,
}

impl Lfg {
	pub fn new(seed_bytes: &[u8]) -> Self {
		let mut buffer = [0u32; LFG_K];
		for i in 0..LFG_SEED_SIZE {
			let o = i * 4;
			buffer[i] = u32::from_be_bytes([
				seed_bytes[o],
				seed_bytes[o + 1],
				seed_bytes[o + 2],
				seed_bytes[o + 3],
			]);
		}
		for i in LFG_SEED_SIZE..LFG_K {
			buffer[i] = buffer[i - 17].wrapping_shl(23) ^ (buffer[i - 16] >> 9) ^ buffer[i - 1];
		}
		for b in buffer.iter_mut() {
			*b = swap32((*b & 0xff00ffff) | ((*b >> 2) & 0x00ff0000));
		}
		let mut lfg = Lfg {
			buffer,
			position: 0,
		};
		for _ in 0..4 {
			lfg.forward();
		}
		lfg
	}

	fn forward(&mut self) {
		for i in 0..LFG_J {
			self.buffer[i] ^= self.buffer[i + LFG_K - LFG_J];
		}
		for i in LFG_J..LFG_K {
			self.buffer[i] ^= self.buffer[i - LFG_J];
		}
	}

	/// バイト数だけ前方へ進める。
	pub fn forward_bytes(&mut self, count: usize) {
		self.position += count;
		while self.position >= LFG_K * 4 {
			self.forward();
			self.position -= LFG_K * 4;
		}
	}

	/// count バイトのストリームを生成する。
	pub fn bytes(&mut self, count: usize) -> Vec<u8> {
		let mut out = vec![0u8; count];
		for o in out.iter_mut() {
			*o = ((self.buffer[self.position >> 2] >> ((self.position & 3) * 8)) & 0xff) as u8;
			self.position += 1;
			if self.position == LFG_K * 4 {
				self.forward();
				self.position = 0;
			}
		}
		out
	}
}

// ---------- 圧縮側: LFG 検出・シード導出 ----------

enum SeedU32 {
	NotEnough,
	FilterFail,
	InitFail,
	Ok([u32; LFG_SEED_SIZE]),
}

/// LFG 検出・シード導出用のジェネレータ。
pub struct LaggedFibonacciGenerator {
	buffer: [u32; LFG_K],
	position_bytes: usize,
}

impl Default for LaggedFibonacciGenerator {
	fn default() -> Self {
		Self::new()
	}
}

impl LaggedFibonacciGenerator {
	pub fn new() -> Self {
		Self {
			buffer: [0u32; LFG_K],
			position_bytes: 0,
		}
	}

	pub fn forward(&mut self) {
		for i in 0..LFG_J {
			self.buffer[i] ^= self.buffer[i + LFG_K - LFG_J];
		}
		for i in LFG_J..LFG_K {
			self.buffer[i] ^= self.buffer[i - LFG_J];
		}
	}

	pub fn backward(&mut self, start_word: usize, end_word: usize) {
		let loop_end = LFG_J.max(start_word);
		let mut i = end_word.min(LFG_K);
		while i > loop_end {
			self.buffer[i - 1] ^= self.buffer[i - 1 - LFG_J];
			i -= 1;
		}
		let mut i = end_word.min(LFG_J);
		while i > start_word {
			self.buffer[i - 1] ^= self.buffer[i - 1 + LFG_K - LFG_J];
			i -= 1;
		}
	}

	fn initialize(&mut self, check_existing_data: bool) -> bool {
		for i in LFG_SEED_SIZE..LFG_K {
			let calculated = self.buffer[i - 17].wrapping_shl(23)
				^ (self.buffer[i - 16] >> 9)
				^ self.buffer[i - 1];
			if check_existing_data {
				let actual =
					(self.buffer[i] & 0xff00ffff) | (self.buffer[i].wrapping_shl(2) & 0x00fc0000);
				if (calculated & 0xfffcffff) != actual {
					return false;
				}
			}
			self.buffer[i] = calculated;
		}
		for b in self.buffer.iter_mut() {
			*b = swap32((*b & 0xff00ffff) | ((*b >> 2) & 0x00ff0000));
		}
		for _ in 0..4 {
			self.forward();
		}
		true
	}

	fn reinitialize(&mut self) -> (Option<[u32; LFG_SEED_SIZE]>, bool) {
		for _ in 0..4 {
			self.backward(0, LFG_K);
		}
		for b in self.buffer.iter_mut() {
			*b = swap32(*b);
		}
		for i in 0..LFG_SEED_SIZE {
			self.buffer[i] = (self.buffer[i] & 0xff00ffff)
				| (self.buffer[i].wrapping_shl(2) & 0x00fc0000)
				| ((self.buffer[i + 16] ^ self.buffer[i + 15]).wrapping_shl(9) & 0x00030000);
		}
		let mut seed = [0u32; LFG_SEED_SIZE];
		for i in 0..LFG_SEED_SIZE {
			seed[i] = swap32(self.buffer[i]);
		}
		let ok = self.initialize(true);
		(Some(seed), ok)
	}

	fn get_byte(&mut self) -> u8 {
		let result = ((self.buffer[self.position_bytes >> 2] >> ((self.position_bytes & 3) * 8))
			& 0xff) as u8;
		self.position_bytes += 1;
		if self.position_bytes == LFG_K * 4 {
			self.forward();
			self.position_bytes = 0;
		}
		result
	}

	fn get_seed_u32(&mut self, u32_data: &[u32], data_offset_words: usize) -> SeedU32 {
		if u32_data.len() < LFG_K {
			return SeedU32::NotEnough;
		}
		for &x in &u32_data[..LFG_K] {
			if (x & 0x00c00000) != ((x >> 2) & 0x00c00000) {
				return SeedU32::FilterFail;
			}
		}
		let modv = data_offset_words % LFG_K;
		let div = data_offset_words / LFG_K;
		for i in 0..(LFG_K - modv) {
			self.buffer[i + modv] = swap32(u32_data[i]);
		}
		for i in 0..modv {
			self.buffer[i] = swap32(u32_data[LFG_K - modv + i]);
		}
		self.backward(0, modv);
		for _ in 0..div {
			self.backward(0, LFG_K);
		}
		let (seed, ok) = self.reinitialize();
		if !ok {
			return SeedU32::InitFail;
		}
		for _ in 0..div {
			self.forward();
		}
		match seed {
			Some(s) => SeedU32::Ok(s),
			None => SeedU32::InitFail,
		}
	}

	/// data の先頭から LFG パディングとして一致するバイト数と、そのシードを返す。
	pub fn get_seed(
		&mut self,
		data: &[u8],
		size: usize,
		data_offset: usize,
	) -> (usize, Option<[u32; LFG_SEED_SIZE]>) {
		let bytes_to_skip = ((data_offset + 3) & !3) - data_offset;
		// 移植元の境界（wordCount が負になるケース）を安全化する
		if size < bytes_to_skip + 4 {
			return (0, None);
		}
		let word_count = (size - bytes_to_skip) / 4;
		let mut u32_data = vec![0u32; word_count];
		for (i, w) in u32_data.iter_mut().enumerate() {
			let o = i * 4 + bytes_to_skip;
			*w = u32::from_be_bytes([data[o], data[o + 1], data[o + 2], data[o + 3]]);
		}
		let u32_data_offset = (data_offset + bytes_to_skip) / 4;
		let seed = match self.get_seed_u32(&u32_data, u32_data_offset) {
			SeedU32::Ok(s) => s,
			_ => return (0, None),
		};
		self.position_bytes = data_offset % (LFG_K * 4);
		let mut count = 0;
		while count < size && self.get_byte() == data[count] {
			count += 1;
		}
		(count, Some(seed))
	}
}

/// シードを 68 バイトにエンコード（リトルエンディアン）。
pub fn seed_bytes(seed: &[u32; LFG_SEED_SIZE]) -> [u8; LFG_SEED_BYTES] {
	let mut out = [0u8; LFG_SEED_BYTES];
	for (i, v) in seed.iter().enumerate() {
		out[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
	}
	out
}

/// 32bit ビッグエンディアン。
#[inline]
pub fn u32be(value: u32) -> [u8; 4] {
	value.to_be_bytes()
}
