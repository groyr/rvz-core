//! rvz クレートのエラー型。

/// RVZ コンテナ処理のエラー。
#[derive(Debug, PartialEq, Eq)]
pub enum RvzError {
	/// packing 入力サイズが packedSize と一致しない
	PackingSizeMismatch,
	/// packing レコードが途中で終わっている
	TruncatedRecord,
	/// padding レコードのシードが不足
	TruncatedSeed,
	/// padding レコードの展開サイズが範囲外
	PaddingOverflow,
	/// packing レコードのサイズが不正
	BadRecord,
	/// packing 入力をすべて消費できなかった
	UnconsumedInput,
	/// 展開後のサイズが一致しない
	SizeMismatch,
}

impl std::fmt::Display for RvzError {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		let msg = match self {
			RvzError::PackingSizeMismatch => "RVZ packing 入力サイズが一致しません",
			RvzError::TruncatedRecord => "RVZ packing レコードが途中で終わっています",
			RvzError::TruncatedSeed => "RVZ padding レコードのシードが不足しています",
			RvzError::PaddingOverflow => "RVZ padding レコードの展開サイズが範囲外です",
			RvzError::BadRecord => "RVZ packing レコードのサイズが不正です",
			RvzError::UnconsumedInput => "RVZ packing 入力をすべて消費できませんでした",
			RvzError::SizeMismatch => "RVZ グループの展開サイズが一致しません",
		};
		write!(f, "{}", msg)
	}
}

impl std::error::Error for RvzError {}
