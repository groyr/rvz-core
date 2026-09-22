// gc-disc の単体テスト（ハードウェア不要）。
// 既知の seed で正当なスクランブル済みブロックを作り、解除と EDC 検証を確認する。
// 移植元 gc-live-disc-server/tests/test_descramble.py と同等の内容。

use gc_disc::*;

const SEED: u16 = 0x1234;
const LBA: u32 = 32;

fn payload() -> Vec<u8> {
	(0..USER_SECTOR_SIZE)
		.map(|i| ((i * 7 + 3) & 0xFF) as u8)
		.collect()
}

#[test]
fn make_and_unscramble() {
	let p = payload();
	let block = make_test_block(SEED, &p, LBA);
	assert_eq!(block.len(), RAW_BLOCK_SIZE);

	let mut d = Descrambler::new(None);
	d.set_seed((LBA as usize / SECTORS_PER_BLOCK) % SECTORS_PER_BLOCK, SEED); // 探索をスキップ
	let out = d.unscramble_block(LBA, &block, true).unwrap();
	assert_eq!(out.len(), BLOCK_SIZE);
	for k in 0..SECTORS_PER_BLOCK {
		assert_eq!(
			&out[k * USER_SECTOR_SIZE..(k + 1) * USER_SECTOR_SIZE],
			&p[..],
			"sector {}",
			k
		);
	}
}

#[test]
fn test_seed_fn() {
	let block = make_test_block(SEED, &payload(), LBA);
	let first = &block[..RAW_SECTOR_SIZE];
	assert!(test_seed(first, SEED));
	assert!(!test_seed(first, (SEED + 1) & 0x7FFF));
}

#[test]
fn edc_and_lfsr_deterministic() {
	assert_eq!(edc_calc(0, &[0u8; 16]), edc_calc(0, &[0u8; 16]));
	assert_eq!(lfsr_stream(0x1234).len(), USER_SECTOR_SIZE);
	assert_ne!(lfsr_stream(0x1234), lfsr_stream(0x1235));
}

#[test]
fn find_seed_small_range() {
	// 探索範囲を狭めて、seed 探索が機能することを確認する。
	let block = make_test_block(0x0005, &payload(), LBA);
	let mut d = Descrambler::new(None);
	d.set_max_seed_search(0x0040); // 0x0005 を探索できる範囲
	let phase = (LBA as usize / SECTORS_PER_BLOCK) % SECTORS_PER_BLOCK;
	assert_eq!(d.find_seed(&block, phase), Some(0x0005));
	let out = d.unscramble_block(LBA, &block, true).unwrap();
	assert_eq!(&out[..USER_SECTOR_SIZE], &payload()[..]);
}

#[test]
fn seed_cache_roundtrip() {
	let dir = std::env::temp_dir().join(format!("gc-disc-test-{}", std::process::id()));
	std::fs::create_dir_all(&dir).unwrap();
	let path = dir.join("seeds.json");
	let _ = std::fs::remove_file(&path);

	let mut a = Descrambler::new(Some(path.clone()));
	a.set_seed(3, 0x0ABC);
	a.save();

	let b = Descrambler::new(Some(path));
	assert_eq!(b.seed(3), Some(0x0ABC));
}
