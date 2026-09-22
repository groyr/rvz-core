//! rvz-core の WebAssembly バインディング（push 型デコーダ）。
//!
//! wasm から読みに行かず、JS が読んだバイトを `rvz_decoder_feed` で渡す方式。
//! 手順:
//!   1. `rvz_decoder_new(file_size)`
//!   2. `rvz_decoder_request` が 1 を返す間、offset/len を読み `rvz_decoder_feed` に渡す
//!   3. 各 feed 後に `rvz_decoder_take_output` で (offset, data) を取り出して書き込む
//!
//! wasm-bindgen は使わず、生の線形メモリ + extern "C" で提供する。

use std::os::raw::c_int;

use rvz::PushDecoder;

/// WASM 線形メモリを `len` バイト確保し、ポインタを返す。
/// u64 の書き込みに備えて 8 バイト境界に整列させる。
#[no_mangle]
pub extern "C" fn rvz_alloc(len: usize) -> *mut u8 {
	let words = len.div_ceil(8).max(1);
	let mut buf = Vec::<u64>::with_capacity(words);
	let ptr = buf.as_mut_ptr() as *mut u8;
	core::mem::forget(buf);
	ptr
}

/// `rvz_alloc` で確保した領域を解放する（`len` は確保時と同じ値）。
///
/// # Safety
/// `ptr` は `rvz_alloc(len)` が返したポインタで、一度だけ解放されること。
#[no_mangle]
pub unsafe extern "C" fn rvz_free(ptr: *mut u8, len: usize) {
	if !ptr.is_null() {
		let words = len.div_ceil(8).max(1);
		drop(Vec::from_raw_parts(ptr as *mut u64, 0, words));
	}
}

/// デコーダを生成する。
#[no_mangle]
pub extern "C" fn rvz_decoder_new(file_size: u64) -> *mut PushDecoder {
	Box::into_raw(Box::new(PushDecoder::new(file_size)))
}

/// デコーダを解放する。
///
/// # Safety
/// `dec` は `rvz_decoder_new` が返したポインタであること。
#[no_mangle]
pub unsafe extern "C" fn rvz_decoder_free(dec: *mut PushDecoder) {
	if !dec.is_null() {
		drop(Box::from_raw(dec));
	}
}

/// 出力 ISO サイズ（ヘッダ解析前は 0）。
///
/// # Safety
/// `dec` は有効なポインタであること。
#[no_mangle]
pub unsafe extern "C" fn rvz_decoder_output_size(dec: *const PushDecoder) -> u64 {
	(*dec).output_size()
}

/// 完了していれば 1。
///
/// # Safety
/// `dec` は有効なポインタであること。
#[no_mangle]
pub unsafe extern "C" fn rvz_decoder_is_done(dec: *const PushDecoder) -> c_int {
	if (*dec).is_done() {
		1
	} else {
		0
	}
}

/// 次の読み出し要求を取り出す。返り値 1=要求あり（offset/len を書き込み）、0=完了。
///
/// # Safety
/// `dec`/`out_offset`/`out_len` は有効なポインタであること。
#[no_mangle]
pub unsafe extern "C" fn rvz_decoder_request(
	dec: *const PushDecoder,
	out_offset: *mut u64,
	out_len: *mut usize,
) -> c_int {
	match (*dec).request() {
		Some(req) => {
			*out_offset = req.offset;
			*out_len = req.len;
			1
		}
		None => 0,
	}
}

/// 直前の要求に対するバイト列を渡す。返り値 0=成功、-1=失敗。
///
/// # Safety
/// `dec` は有効、`ptr` は `len` バイト有効であること。
#[no_mangle]
pub unsafe extern "C" fn rvz_decoder_feed(
	dec: *mut PushDecoder,
	ptr: *const u8,
	len: usize,
) -> c_int {
	let bytes = if len == 0 {
		&[][..]
	} else {
		core::slice::from_raw_parts(ptr, len)
	};
	match (*dec).feed(bytes) {
		Ok(()) => 0,
		Err(_) => -1,
	}
}

/// 次の出力を取り出して `dst` へコピーする。
/// 返り値: コピーしたバイト数（0 以上）。出力なしは -1。`cap` 不足は負値（必要サイズの -1 倍）。
///
/// # Safety
/// `dec`/`dst`/`out_offset` は有効であること。
#[no_mangle]
pub unsafe extern "C" fn rvz_decoder_take_output(
	dec: *mut PushDecoder,
	dst: *mut u8,
	cap: usize,
	out_offset: *mut u64,
) -> i64 {
	let Some((off, data)) = (*dec).take_output() else {
		return -1;
	};
	if data.len() > cap {
		// 容量不足: 呼び出し側が大きいバッファで再試行できるよう、必要サイズを負値で返す
		return -((data.len() as i64) + 1);
	}
	core::ptr::copy_nonoverlapping(data.as_ptr(), dst, data.len());
	*out_offset = off;
	data.len() as i64
}

/// 直近のエラー文字列を `dst` へコピーし、長さを返す。
///
/// # Safety
/// `dec`/`dst` は有効であること。
#[no_mangle]
pub unsafe extern "C" fn rvz_decoder_last_error(
	dec: *const PushDecoder,
	dst: *mut u8,
	cap: usize,
) -> usize {
	let msg = (*dec).error().unwrap_or("");
	let n = msg.len().min(cap);
	core::ptr::copy_nonoverlapping(msg.as_ptr(), dst, n);
	n
}

/// ディスクヘッダ（dhead, 最大 0x80 バイト）を `dst` へコピーし、長さを返す。
///
/// # Safety
/// `dec`/`dst` は有効であること。
#[no_mangle]
pub unsafe extern "C" fn rvz_decoder_dhead(dec: *const PushDecoder, dst: *mut u8, cap: usize) -> usize {
	let Some(dh) = (*dec).dhead() else {
		return 0;
	};
	let n = dh.len().min(cap);
	core::ptr::copy_nonoverlapping(dh.as_ptr(), dst, n);
	n
}
