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
    - 検証: MKWii の RVZ（2.65GB）→ 参照 ISO（4.48GB）と **MD5 完全一致**（`1942f9c1…`）
  - **エンコーダ（ISO→RVZ）**: パーティション検出・LFG packing・ハッシュ再計算・例外リスト・zstd
    - 検証: MKWii の ISO を圧縮 → TS 実装の level3 出力と **サイズ・MD5 完全一致**
      （`2821311242` B / `386ea195…`）。自作 RVZ の再展開も ISO MD5 一致
  - `encode` feature（既定）は zstd 圧縮を使うためネイティブ前提（C ツールチェーンが必要）。wasm では無効化する
- `crates/rvz-cli` … 検証用 CLI
  - `rvz-cli decode <input.rvz>`（ISO 全体の MD5 を表示）
  - `rvz-cli encode <input.iso> <out.rvz> [level]`

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
