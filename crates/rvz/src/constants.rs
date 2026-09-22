// Wii ディスクフォーマット／RVZ コンテナの共有定数。

/// パーティションブロック（0x8000 = 32KiB）内のレイアウト
pub const BLOCK_HEADER_SIZE: usize = 0x400;
pub const BLOCK_DATA_SIZE: usize = 0x7c00;
pub const BLOCK_TOTAL_SIZE: usize = 0x8000;
pub const BLOCKS_PER_GROUP: usize = 64;

/// ハッシュツリー（h0 は 31 個 × 0x400 チャンク、h1/h2 は各 8 個、padding はゼロ）
pub const H0_ARRAY_SIZE: usize = 31 * 20;

/// RVZ 管理テーブルのエントリサイズ
pub const RAW_ENTRY_SIZE: usize = 0x18;
pub const GROUP_ENTRY_SIZE: usize = 0x0c;
pub const PART_ENTRY_SIZE: usize = 0x30;
pub const PART_DATA_ENTRY_SIZE: usize = 0x10;

/// ディスクヘッダ（先頭 0x80）と Wii 判別マジック
pub const DISC_HEADER_SIZE: usize = 0x80;
pub const WII_MAGIC: u32 = 0x5d1c9ea3;

/// RVZ のチャンクサイズ（128KiB = 0x20000）。管理テーブルとグループの単位
pub const CHUNK_SIZE: usize = 0x20000;

/// データレイアウトのグループ総サイズ（2MiB）
pub const GROUP_TOTAL_SIZE: usize = 0x200000;

/// 32bit バイトスワップ（ビッグエンディアン ↔ リトルエンディアン）
#[inline]
pub const fn swap32(value: u32) -> u32 {
	((value & 0xff) << 24)
		| ((value & 0xff00) << 8)
		| ((value >> 8) & 0xff00)
		| ((value >> 24) & 0xff)
}

/// `alignment` の倍数へ切り下げる。
#[inline]
pub const fn align_down(value: u64, alignment: u64) -> u64 {
	value - (value % alignment)
}

/// `alignment` の倍数へ切り上げる。
#[inline]
pub const fn align_up(value: u64, alignment: u64) -> u64 {
	align_down(value + alignment - 1, alignment)
}

/// 4 バイト境界へ切り上げる。
#[inline]
pub const fn align4(value: usize) -> usize {
	(value + 3) & !3
}
