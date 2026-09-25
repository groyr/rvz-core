//! エンコーダのストリーミング（--follow 相当）と ISO 往復のテスト。
//! 実ディスクを使わず、合成 ISO と段階的に読める ReadAt で検証する。

use std::io;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use rvz::{decompress_rvz, encode_iso_to_rvz, ReadAt, WriteAt};

fn put_be32(buf: &mut [u8], off: usize, v: u32) {
	buf[off..off + 4].copy_from_slice(&v.to_be_bytes());
}

/// メモリ上の ReadAt。
struct MemReader {
	data: Vec<u8>,
}

impl ReadAt for MemReader {
	fn read_at(&self, offset: u64, len: usize) -> io::Result<Vec<u8>> {
		let start = offset as usize;
		if start >= self.data.len() {
			return Ok(Vec::new());
		}
		let end = (start + len).min(self.data.len());
		Ok(self.data[start..end].to_vec())
	}
}

/// メモリ上の WriteAt（範囲外はゼロ拡張）。
struct MemWriter {
	data: Vec<u8>,
}

impl WriteAt for MemWriter {
	fn write_at(&mut self, offset: u64, data: &[u8]) -> io::Result<()> {
		let end = offset as usize + data.len();
		if self.data.len() < end {
			self.data.resize(end, 0);
		}
		self.data[offset as usize..end].copy_from_slice(data);
		Ok(())
	}
}

/// 書き込みを捨てる WriteAt（ストリーミング検証用）。
struct SinkWriter;

impl WriteAt for SinkWriter {
	fn write_at(&mut self, _offset: u64, _data: &[u8]) -> io::Result<()> {
		Ok(())
	}
}

/// 合成 Wii ISO を作る。`parts` は (パーティション先頭, データサイズ)。
/// パーティションヘッダの magic と data_offset/data_size だけを正しく埋める。
fn synthetic_wii_iso(iso_size: usize, parts: &[(u64, u64)]) -> Vec<u8> {
	let mut iso = vec![0u8; iso_size];
	// ディスクヘッダ: Wii マジック（disc_type=2 判定に使う）
	put_be32(&mut iso, 0x18, 0x5d1c9ea3);
	// パーティション表ヘッダ @0x40000（group0 のみ）
	put_be32(&mut iso, 0x40000, parts.len() as u32);
	put_be32(&mut iso, 0x40004, 0x0001_0008); // table offset = 0x40020（4バイト単位）
	let table = 0x40020usize;
	for (i, (poff, _)) in parts.iter().enumerate() {
		put_be32(&mut iso, table + i * 8, (*poff / 4) as u32);
		put_be32(&mut iso, table + i * 8 + 4, 1); // type=game
	}
	for (poff, dsize) in parts {
		let p = *poff as usize;
		put_be32(&mut iso, p, 0x0001_0001); // partition magic
		put_be32(&mut iso, p + 0x2b8, 0x2000); // data_offset = 0x8000（4バイト単位）
		put_be32(&mut iso, p + 0x2bc, (*dsize as u32) / 4); // data_size
	}
	// 内容を非自明にして圧縮を機能させる
	for (i, b) in iso.iter_mut().enumerate() {
		if *b == 0 {
			*b = ((i * 31 + 7) & 0xff) as u8;
		}
	}
	iso
}

/// 指定 `available` バイトまでしか読めない ReadAt（FollowReader 相当）。
struct GatedReader {
	data: Vec<u8>,
	available: Arc<AtomicU64>,
}

impl ReadAt for GatedReader {
	fn read_at(&self, offset: u64, len: usize) -> io::Result<Vec<u8>> {
		let want = offset + len as u64;
		let deadline = Instant::now() + Duration::from_secs(10);
		while self.available.load(Ordering::SeqCst) < want {
			if Instant::now() > deadline {
				return Err(io::Error::new(io::ErrorKind::TimedOut, "read timeout"));
			}
			std::thread::sleep(Duration::from_millis(5));
		}
		let start = offset as usize;
		let end = (start + len).min(self.data.len());
		Ok(self.data[start..end].to_vec())
	}
}

#[test]
fn gc_raw_roundtrip() {
	// パーティション無し（disc_type=1）の ISO が encode→decode で完全一致する
	let iso_size = 0x40_0000usize; // 4MiB
	let mut iso = vec![0u8; iso_size];
	for (i, b) in iso.iter_mut().enumerate() {
		*b = ((i * 13 + 5) & 0xff) as u8;
	}
	put_be32(&mut iso, 0x18, 0); // Wii マジックではない

	let reader = MemReader { data: iso.clone() };
	let mut writer = MemWriter { data: Vec::new() };
	encode_iso_to_rvz(iso_size as u64, 3, &reader, &mut writer, |_, _| {}).unwrap();

	let rvz_bytes = writer.data;
	let file_size = rvz_bytes.len() as u64;
	let rvz_reader = MemReader { data: rvz_bytes };
	let mut out = MemWriter { data: Vec::new() };
	let decoded = decompress_rvz(file_size, &rvz_reader, |off, data| {
		out.write_at(off, data).unwrap();
	})
	.unwrap();

	assert_eq!(decoded, iso_size as u64);
	assert_eq!(out.data, iso);
}

#[test]
fn wii_partition_roundtrip() {
	// パーティション付き合成 Wii ISO が encode→decode で完全一致する
	let iso_size = 0x80_0000usize; // 8MiB
	let parts = [(0x8_0000u64, 0x4_0000u64), (0x78_0000u64, 0x2_0000u64)];
	let iso = synthetic_wii_iso(iso_size, &parts);

	let reader = MemReader { data: iso.clone() };
	let mut writer = MemWriter { data: Vec::new() };
	encode_iso_to_rvz(iso_size as u64, 3, &reader, &mut writer, |_, _| {}).unwrap();

	let rvz_bytes = writer.data;
	let file_size = rvz_bytes.len() as u64;
	let rvz_reader = MemReader { data: rvz_bytes };
	let mut out = MemWriter { data: Vec::new() };
	let decoded = decompress_rvz(file_size, &rvz_reader, |off, data| {
		out.write_at(off, data).unwrap();
	})
	.unwrap();

	assert_eq!(decoded, iso_size as u64);
	assert_eq!(out.data, iso);
}

#[test]
fn follow_streams_before_late_partition() {
	// 末端パーティションのヘッダが未到着でも、手前の raw/パーティションを圧縮できる
	let iso_size = 0x80_0000usize; // 8MiB
	let late = 0x78_0000u64; // 末端パーティション
	let parts = [(0x8_0000u64, 0x4_0000u64), (late, 0x2_0000u64)];
	let iso = synthetic_wii_iso(iso_size, &parts);

	// 末端パーティション手前までだけ読める状態にする
	let available = Arc::new(AtomicU64::new(0x70_0000));
	let reader = GatedReader {
		data: iso,
		available: available.clone(),
	};
	let progress = Arc::new(AtomicU64::new(0));
	let progress_thread = progress.clone();

	let handle = std::thread::spawn(move || {
		let mut writer = SinkWriter;
		encode_iso_to_rvz(iso_size as u64, 3, &reader, &mut writer, move |done, _| {
			progress_thread.fetch_max(done, Ordering::SeqCst);
		})
	});

	// 末端パーティションに到達する前に進捗が出ていることを確認（旧実装はここで 0 のまま停止）
	let deadline = Instant::now() + Duration::from_secs(5);
	while progress.load(Ordering::SeqCst) < 0x10_0000 {
		if Instant::now() > deadline {
			panic!(
				"ストリーミング進捗が出ませんでした: done={}",
				progress.load(Ordering::SeqCst)
			);
		}
		std::thread::sleep(Duration::from_millis(10));
	}
	assert!(
		progress.load(Ordering::SeqCst) >= 0x10_0000,
		"末端パーティション到着前に圧縮が進んでいません"
	);

	// 残りを解放して完了させる
	available.store(iso_size as u64, Ordering::SeqCst);
	let rvz_size = handle.join().unwrap().unwrap();
	assert!(rvz_size > 0);
}
