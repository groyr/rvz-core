# rvz-core

GameCube のディスク処理（スクランブル解除・RVZ コンテナ）を Rust で実装したコア。
WebAssembly とネイティブ CLI の両方にビルドでき、複数のアプリから共有できる。

## クレート

- `crates/gc-disc` … Nintendo GameCube のスクランブル解除（descramble）と EDC 検証
  - 出典: friidump (Arep, GPLv2+) の unscrambler を基にした CleanRip
    `source/disc_scramble.c` と同一アルゴリズム
- `crates/rvz` … RVZ コンテナ（GC/Wii ディスクイメージの Zstandard 圧縮コンテナ）
  - packing 層（LFG パディングの pack/unpack）
  - デコーダ（RVZ→ISO）: zstd（ruzstd）+ LFG unpack + ハッシュ再構築 + AES-128-CBC 再暗号化
    - 検証: Wii ディスクの RVZ を展開し、参照 ISO と **MD5 完全一致**
  - エンコーダ（ISO→RVZ）: パーティション検出・LFG packing・ハッシュ再計算・例外リスト・zstd
    - 検証: 同一 ISO の圧縮出力が参照実装と**サイズ・MD5 完全一致**、再展開も ISO と一致
  - `encode` feature（既定）は zstd 圧縮を使うためネイティブ前提（C ツールチェーンが必要）。wasm では無効化する
- `crates/rvz-wasm` … WebAssembly バインディング（**push 型デコーダ**）
  - 非同期 I/O と両立させるため、wasm から読みに行かず「次に読む位置」を要求し、
    JS が読んだバイトを `rvz_decoder_feed` で渡す。出力は `rvz_decoder_take_output` で取り出す
  - `rvz_decoder_new_ex(file_size, delegate_aes)` で **AES 再暗号化を呼び出し側へ委譲**できる
    （平文ブロックと領域キーを返す。ブラウザでは `crypto.subtle` の HW AES を使えて大幅に高速化）
- `crates/rvz-cli` … CLI
  - `rvz-cli decode <input.rvz>`（ISO 全体の MD5 を表示）
  - `rvz-cli decode-push <input.rvz>`（push 型デコーダ版）
  - `rvz-cli encode <input.iso> <out.rvz> [level] [--json] [--follow --iso-size <bytes>]`
    - `--json` で `progress` / `done` / `error` を 1 行 JSON で出力（進捗表示向け）
    - `progress` は `phase`（`encode` / `md5`）と `elapsedMs` を含む。`value` は 0..1
    - `--follow` は成長中の ISO を末尾追従する。Wii のパーティションヘッダは
      **その位置に到達してから読む**ため、ディスク末端側のパーティションがあっても
      圧縮が停止せず、吸い出しと並行して進む

## ビルド・テスト

```sh
cargo test
cargo build --release
```

wasm ターゲット:

```sh
rustup target add wasm32-unknown-unknown
cargo build --release -p rvz-wasm --target wasm32-unknown-unknown
```

デコーダの簡易検証（Node から wasm を駆動）:

```sh
node tools/wasm-decode-check.mjs \
  target/wasm32-unknown-unknown/release/rvz_wasm.wasm <input.rvz>
```

## ライセンス

GPL-2.0-or-later（出典: friidump / CleanRip / Dolphin 系の GPL 実装に由来）。
全文は `LICENSE` を参照。
