//! ランダムアクセス読み出しの抽象（RVZ ファイルなど）。

/// `offset` から `len` バイトを読む。
pub trait ReadAt {
	fn read_at(&self, offset: u64, len: usize) -> std::io::Result<Vec<u8>>;
}

/// メモリ上のスライスからの読み出し（テスト用）。
impl ReadAt for [u8] {
	fn read_at(&self, offset: u64, len: usize) -> std::io::Result<Vec<u8>> {
		let start = offset as usize;
		let end = (start + len).min(self.len());
		if start > self.len() {
			return Ok(Vec::new());
		}
		Ok(self[start..end].to_vec())
	}
}
