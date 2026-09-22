// tools/wasm-decode-check.mjs
//
// rvz-wasm（push 型デコーダ）の wasm ABI を Node で駆動し、RVZ を展開して
// ISO 全体の MD5 を表示する。参照 ISO の MD5 と比較して検証する。
//
// 使い方:
//   cargo build --release -p rvz-wasm --target wasm32-unknown-unknown
//   node tools/wasm-decode-check.mjs <rvz_wasm.wasm> <input.rvz>

import crypto from 'node:crypto';
import fs from 'node:fs';

const wasmPath = process.argv[2];
const rvzPath = process.argv[3];
if (!wasmPath || !rvzPath) {
  console.error('使い方: node tools/wasm-decode-check.mjs <rvz_wasm.wasm> <input.rvz>');
  process.exit(2);
}

const { instance } = await WebAssembly.instantiate(fs.readFileSync(wasmPath));
const ex = instance.exports;
const mem = () => new DataView(ex.memory.buffer);

const fileSize = BigInt(fs.statSync(rvzPath).size);
const fd = fs.openSync(rvzPath, 'r');

const OUT_CAP = 4 * 1024 * 1024;
const outPtr = ex.rvz_alloc(OUT_CAP);
const offPtr = ex.rvz_alloc(8);
const lenPtr = ex.rvz_alloc(8);
let readPtr = ex.rvz_alloc(1);
let readCap = 1;

function ensureRead(len) {
  if (len > readCap) {
    ex.rvz_free(readPtr, readCap);
    readCap = len;
    readPtr = ex.rvz_alloc(len);
  }
}

function readFileAt(offset, len) {
  const buf = Buffer.alloc(len);
  let got = 0;
  while (got < len) {
    const n = fs.readSync(fd, buf, got, len - got, Number(offset) + got);
    if (n <= 0) break;
    got += n;
  }
  return buf.subarray(0, got);
}

const dec = ex.rvz_decoder_new(fileSize);
const md5 = crypto.createHash('md5');
let written = 0;

function drain() {
  for (;;) {
    const n = ex.rvz_decoder_take_output(dec, outPtr, OUT_CAP, offPtr);
    if (n === -1n) return;
    if (n < 0n) throw new Error(`出力バッファ不足: ${-n - 1n} バイト必要`);
    const size = Number(n);
    const data = Buffer.from(new Uint8Array(ex.memory.buffer, outPtr, size));
    md5.update(data);
    written += size;
  }
}

for (;;) {
  const has = ex.rvz_decoder_request(dec, offPtr, lenPtr);
  if (!has) break;
  const dv = mem();
  const offset = dv.getBigUint64(offPtr, true);
  const len = dv.getUint32(lenPtr, true); // wasm32 の usize
  if (len > 0) {
    const bytes = readFileAt(offset, len);
    ensureRead(len);
    new Uint8Array(ex.memory.buffer, readPtr, len).set(bytes);
    if (ex.rvz_decoder_feed(dec, readPtr, len) !== 0) {
      const p = ex.rvz_alloc(256);
      const n = ex.rvz_decoder_last_error(dec, p, 256);
      const msg = Buffer.from(new Uint8Array(ex.memory.buffer, p, n)).toString('utf8');
      throw new Error(`feed 失敗: ${msg}`);
    }
  } else if (ex.rvz_decoder_feed(dec, readPtr, 0) !== 0) {
    throw new Error('feed(0) 失敗');
  }
  drain();
}
drain();

const isoSize = ex.rvz_decoder_output_size(dec);
console.log(`iso_size=${isoSize} bytes_written=${written}`);
console.log(`md5=${md5.digest('hex')}`);
ex.rvz_decoder_free(dec);
fs.closeSync(fd);
