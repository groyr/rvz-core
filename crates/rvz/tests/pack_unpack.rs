// rvz の pack/unpack と LFG のテスト。
// LFG と packing は rvz-converter（TS 実装）が生成した参照ベクトルと一致することを確認する。

use rvz::constants::BLOCK_TOTAL_SIZE;
use rvz::lfg::{seed_bytes, Lfg, LFG_SEED_BYTES};
use rvz::{rvz_pack_chunk, unpack};

fn hex(s: &str) -> Vec<u8> {
	(0..s.len())
		.step_by(2)
		.map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
		.collect()
}

fn seed_words() -> [u32; 17] {
	std::array::from_fn(|i| 0x01020304u32.wrapping_add((i as u32).wrapping_mul(0x11111111)))
}

// rvz-converter web/src/core/lfg.ts が生成した参照ベクトル
const SEED_HEX: &str = "0403020115141312262524233736353448474645595857566a6968677b7a79788c8b8a899d9c9b9aaeadacabbfbebdbcd0cfcecde1e0dfdef2f1f0ef0303020114141312";
const LFG_BYTES_HEX: &str = "c68df12f8a89d0dfce8ace7c67c7124b597573c78fc49392b16466b6e4103030ad67fd54a8121823a3cc127eaab5ae35083f39197292e216202b384b8282f1c3";

#[test]
fn lfg_matches_ts_reference() {
	let seed = hex(SEED_HEX);
	assert_eq!(seed.len(), LFG_SEED_BYTES);
	let mut lfg = Lfg::new(&seed);
	lfg.forward_bytes(1000);
	let out = lfg.bytes(64);
	assert_eq!(out, hex(LFG_BYTES_HEX));
}

#[test]
fn unpack_literal_record() {
	// リテラルレコードのみの packing 入力を展開する
	let payload = [1u8, 2, 3, 4];
	let mut packed = Vec::new();
	packed.extend_from_slice(&(payload.len() as u32).to_be_bytes());
	packed.extend_from_slice(&payload);
	let out = unpack(&packed, payload.len(), 0, packed.len()).unwrap();
	assert_eq!(out, payload);
}

#[test]
fn pack_matches_ts_reference() {
	// 全面 LFG パディングの 1 ウィンドウを pack した結果を TS 実装と一致させる。
	// TS: rvzPackChunk(createLfg(seedBytes(seed)).bytes(0x8000), 0) => 72 バイト
	// 注意: 埋め込まれるシードは「検出されたシード」であり、元シードとは未検証ビットが異なり得る
	const PACK_HEX: &str = "800080000402020115141312262524233736353448474645595857566a6968677b7a79788c8b8a899d9c9b9aaeadacabbfbebdbcd0cfcecde1e0dfdef2f1f0ef0303020114141312";
	let sb = seed_bytes(&seed_words());
	let pad = Lfg::new(&sb).bytes(BLOCK_TOTAL_SIZE);

	let (packed, packed_size) = rvz_pack_chunk(&pad, 0);
	assert_eq!(packed_size, 72);
	assert_eq!(packed, hex(PACK_HEX));

	let out = unpack(&packed, BLOCK_TOTAL_SIZE, 0, packed_size).unwrap();
	assert_eq!(out, pad);
}

#[test]
fn pack_unpack_roundtrip_with_lfg_padding() {
	// 実パディングに相当する LFG ストリームを生成し、pack -> unpack の往復を確認する
	let sb = seed_bytes(&seed_words());

	let literal_prefix = BLOCK_TOTAL_SIZE;
	let pad_len = BLOCK_TOTAL_SIZE;
	let literal_suffix = BLOCK_TOTAL_SIZE;
	let offset = literal_prefix as u64;

	let mut data = vec![0u8; literal_prefix + pad_len + literal_suffix];
	for (i, b) in data.iter_mut().enumerate().take(literal_prefix) {
		*b = ((i * 13 + 7) & 0xff) as u8;
	}
	// パディングは 0x8000 境界に置き、位相 0 の LFG ストリームとする
	let mut lfg = Lfg::new(&sb);
	lfg.forward_bytes(offset as usize % BLOCK_TOTAL_SIZE);
	let pad = lfg.bytes(pad_len);
	data[literal_prefix..literal_prefix + pad_len].copy_from_slice(&pad);
	for (i, b) in data[literal_prefix + pad_len..].iter_mut().enumerate() {
		*b = ((i * 7 + 3) & 0xff) as u8;
	}

	let (packed, packed_size) = rvz_pack_chunk(&data, 0);
	assert!(packed_size > 0, "LFG パディングが検出されませんでした");
	let out = unpack(&packed, data.len(), 0, packed_size).unwrap();
	assert_eq!(out, data);
}
