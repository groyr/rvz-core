# rvz-core

GameCube 関連のディスク処理を PC / Pi / ブラウザで共有するための Rust コア。

複数リポジトリ（`rvz-converter` / `GC Ripper` / `gc-live-disc-server` / `CleanRip`）に
分散していた実装を、ここへ単一ソースとして集約する。

## クレート

- `crates/gc-disc` … Nintendo GameCube のスクランブル解除（descramble）と EDC 検証
  - 出典: friidump (Arep, GPLv2+) の unscrambler → CleanRip `source/disc_scramble.c`
    → gc-live-disc-server `descramble.py` と同一アルゴリズム

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
