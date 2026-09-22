//! 暗号・ハッシュの薄いラッパ（RVZ の AES-128-CBC と SHA-1）。

use aes::cipher::block_padding::NoPadding;
use aes::cipher::{BlockDecryptMut, BlockEncryptMut, KeyIvInit};
use aes::Aes128;
use cbc::{Decryptor, Encryptor};
use sha1::{Digest, Sha1};

/// AES-128-CBC 暗号化（無パディング）。入力長は 16 の倍数であること。
pub fn aes128_cbc_encrypt(key: &[u8], iv: &[u8], data: &[u8]) -> Vec<u8> {
	let mut buf = data.to_vec();
	let enc = Encryptor::<Aes128>::new(key.into(), iv.into());
	enc.encrypt_padded_mut::<NoPadding>(&mut buf, data.len())
		.expect("AES-CBC 暗号化に失敗（len が 16 の倍数でない等）");
	buf
}

/// AES-128-CBC 復号（無パディング）。入力長は 16 の倍数であること。
pub fn aes128_cbc_decrypt(key: &[u8], iv: &[u8], data: &[u8]) -> Vec<u8> {
	let mut buf = data.to_vec();
	let dec = Decryptor::<Aes128>::new(key.into(), iv.into());
	dec.decrypt_padded_mut::<NoPadding>(&mut buf)
		.expect("AES-CBC 復号に失敗（len が 16 の倍数でない等）");
	buf
}

/// `data[offset..offset+length]` の SHA-1 を `out[out_offset..out_offset+20]` に書く。
pub fn sha1_into(data: &[u8], offset: usize, length: usize, out: &mut [u8], out_offset: usize) {
	let digest = Sha1::digest(&data[offset..offset + length]);
	out[out_offset..out_offset + 20].copy_from_slice(&digest);
}
