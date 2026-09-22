# rvz-core

GameCube 関連のディスク処理を PC / Pi / ブラウザで共有するための Rust コア。

複数リポジトリ（`rvz-converter` / `GC Ripper` / `gc-live-disc-server` / `CleanRip`）に
分散していた実装を、ここへ単一ソースとして集約する。

## クレート

- `crates/gc-disc` … Nintendo GameCube のスクランブル解除（descramble）と EDC 検証
  - 出典: friidump (Arep, GPLv2+) の unscrambler → CleanRip `source/disc_scramble.c`
    → gc-live-disc-server `descramble.py` と同一アルゴリズム
- `crates/rvz` … RVZ コンテナ（Wii ディスクイメージの Zstandard 圧縮コンテナ）
  - packing 層（LFG パディングの pack/unpack）: TS 実装の参照ベクトルと一致（テスト済み）
  - **デコーダ（RVZ→ISO）**: zstd（ruzstd）+ LFG unpack + ハッシュ再構築 + AES-128-CBC 再暗号化
    - **実機検証**: MKWii の RVZ（2.65GB）を展開し、参照 ISO（4.48GB）と **MD5 完全一致**
      （`1942f9c1…`）
  - 予定: エンコーダ（ISO→RVZ）と wasm / CLI バインディング
- `crates/rvz-cli` … 検証用 CLI（`rvz-cli decode <input.rvz>` で ISO の MD5 を表示）

## 方針

- 単一ソースから用途別にビルドする:
  - ブラウザ = WebAssembly
  - Pi / PC = ネイティブ CLI（Go サーバから呼ぶ）
- 既存実装（TS / Python / C）との**数値一致**を最優先で検証する

## ビルド・テスト

```sh
cargo test
cargo build --release
```

wasm ターゲット:

```sh
rustup target add wasm32-unknown-unknown
cargo build --release --target wasm32-unknown-unknown
```

## ライセンス

GPL-2.0-or-later（出典: friidump / CleanRip / Dolphin 系の GPL 実装に由来）
