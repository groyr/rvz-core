# AGENTS.md

## プロジェクト概要

GameCube 関連のディスク処理（スクランブル解除・RVZ コンテナ・解析）を、
PC / Pi / ブラウザで共有するための **Rust コア**。

これまで `rvz-converter`（ブラウザ TS）・`GC Ripper`（Node TS）・`gc-live-disc-server`（Python）・
`CleanRip`/`friidump`/`swiss-gc`（C）に重複していた実装を、ここへ一本化する（S2 構想）。

## 役割分担（関連リポジトリ）

| リポジトリ | 役割 |
|---|---|
| `groyr/rvz-core`（本リポジトリ） | ディスク処理の Rust 単一ソース（wasm / ネイティブ CLI） |
| `groyr/rvz-converter` | ブラウザ RVZ 変換（本コアを wasm で利用） |
| `groyr/gc-ripper` | Pi 吸い出しパイプライン（本コアの CLI を利用） |
| `groyr/gc-live-disc-server` | OmniDrive 読み出し（descramble を本コアへ寄せる） |
| `groyr/cleanrip` / `groyr/swiss-gc` | GC 実機側（libogc2 のため C のまま） |

## コマンド

```sh
cargo test
cargo build --release
cargo build --release --target wasm32-unknown-unknown   # 要 rustup target add
cargo fmt
cargo clippy
```

## コーディング規約

- コメント・ログ・エラーは日本語 / 変数・関数名は英語
- 既存実装（C / Python / TS）と**数値一致**を最優先（テストで担保）
- 出典を各ファイル冒頭に明記する（GPL 由来の帰属を維持）

## バージョン管理

- jj (Jujutsu) で運用（`jj git init --colocate`）
- `core.autocrlf=false`・作業ツリーは LF
- 作業前に `jj new`、完了時に `jj describe`（日本語）
- `origin` = GitHub `groyr/rvz-core`

## 既知の制約

- スクランブル解除の seed は disc ごとに異なる（生フレームの EDC で探索・16 周期でキャッシュ）
- GC/PPC 側（libogc2）は Rust へは移植できないため C のまま
