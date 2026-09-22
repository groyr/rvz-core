# AGENTS.md

## プロジェクト概要

GameCube のディスク処理（スクランブル解除・RVZ コンテナ・解析）を Rust で実装したコア。
同じ処理を WebAssembly とネイティブ CLI の両方へビルドでき、複数のアプリから共有できる。

## 構成

- `crates/gc-disc` … GC のスクランブル解除と EDC 検証（出典: friidump / CleanRip `source/disc_scramble.c`）
- `crates/rvz` … RVZ コンテナ（packing / デコーダ / エンコーダ）
- `crates/rvz-wasm` … WebAssembly バインディング（push 型デコーダ）
- `crates/rvz-cli` … CLI（decode / encode / `--json` / `--follow`）

## コマンド

```sh
cargo test
cargo build --release
cargo build --release -p rvz-wasm --target wasm32-unknown-unknown   # 要 rustup target add
cargo fmt
cargo clippy
```

## コーディング規約

- コメント・ログ・エラーは日本語 / 変数・関数名は英語
- 既存実装（C / TS）と**数値一致**を最優先（テストで担保）
- 出典を各ファイル冒頭に明記する（GPL 由来の帰属を維持する）

## バージョン管理

- jj (Jujutsu) で運用（`jj git init --colocate`）
- `core.autocrlf=false`・作業ツリーは LF
- 作業前に `jj new`、完了時に `jj describe`（日本語）
- `origin` = GitHub `groyr/rvz-core`

## 注意（公開リポジトリ）

- 本リポジトリは **公開**。他リポジトリ固有の情報や非公開の事情は書かない
  （ドキュメント・コメント・テストデータとも）。公開されている出典（friidump / CleanRip / Dolphin）のみ参照する

## 既知の制約

- スクランブル解除の seed は disc ごとに異なる（生フレームの EDC で探索・16 周期でキャッシュ）
- GC/PPC 側（libogc2）は Rust へは移植できないため C のまま
