[简体中文](./README.md) | [English](./README.en.md) | 日本語

# LANrurugi

[![CI](https://github.com/thelastfantasy/LANrurugi/actions/workflows/ci.yml/badge.svg?branch=dev)](https://github.com/thelastfantasy/LANrurugi/actions/workflows/ci.yml)
[![CodeQL](https://github.com/thelastfantasy/LANrurugi/actions/workflows/codeql.yml/badge.svg?branch=dev)](https://github.com/thelastfantasy/LANrurugi/actions/workflows/codeql.yml)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#ライセンス)

[ホームページ](https://lrrg.dev) · [デモ](https://demo.lrrg.dev) · [ドキュメント](https://docs.lrrg.dev) · [API リファレンス](https://api.lrrg.dev) · [プラグイン SDK](https://sdk.lrrg.dev)

セルフホスト型の漫画・同人誌ライブラリ管理ツール。Rust バックエンド + React フロントエンドで、
[LANraragi](https://github.com/Difegue/LANraragi) をゼロから書き直したものです。

## LANraragi との関係

ライブラリ構成・タグ構文・HTTP API は LANraragi をそのまま踏襲しているため、既存のクライアント・リーダー・
移行手順はそのまま使えます。同じ漫画ディレクトリをマウントし、バックアップ JSON を読み込むだけです。書き直した
理由は速度と堅牢性にあります。Rust + Tokio によりサムネイル・OCR・画像比較などの CPU 依存処理が全コアを使え、
ダウンロードをプラグインのサンドボックスからホストプロセスへ移したことで、バイト単位の進捗とドメイン別レート
制限が可能になりました。項目ごとの比較（意図的に残した差異を含む）は
[docs/comparison-with-lanraragi.md](./docs/comparison-with-lanraragi.md)（中国語）を参照してください。

## できること

- **ライブラリと閲覧**：カテゴリ、タグ、読書進捗、全文検索とタグ絞り込み。本棚 / グリッド / リスト表示、
  UI は 14 言語。
- **プラグイン**：メタデータ・ダウンロード・ログインの 3 種類が Deno サブプロセスで動作し、名前空間ごとに権限を
  分離。AI ウィザードでサイト向けのプラグインを生成できます。
- **ダウンロード**：バイト単位の進捗、一時停止 / レジューム、ドメイン別の同時実行数とレート制限。キューには待ち時間・
  転送・取り込みがそれぞれ何秒かかったかも表示されます。新刊を検出してルールで判定し、プレビュー後にキューへ入れる購読機能。
- **共有**：アーカイブや単行本を他の人へ共有し、相手はブラウザで読む・ダウンロードする・自分のインスタンスへ
  取り込む、のいずれかを選べます（直接接続・ホールパンチング・リレー。第三者サービス不要）。
- **ページ内翻訳**：OCR → 原文の消去 → 翻訳の組版。クラウド API でもローカルモデルでもよく、フォントは巻単位で
  キャッシュします。
- **電子書籍**：PDF、EPUB、MOBI/AZW3、DjVu、FB2、TXT をアーカイブと同じように登録・閲覧できます。
- **ごみ箱と安全パスワード**：削除はまずごみ箱へ入り、保存期間と容量上限を設定できます。復元するとタグ・
  カテゴリ・ブックマーク・読書進捗も戻ります。完全削除とごみ箱の空化には別途 4 桁の安全パスワードが必要で
  （連続失敗でロック）、ログインパスワードまたは CLI で再設定できます。
- **バックアップと削除のセーフティネット**：cron 式（秒単位まで）による RDB スナップショットと JSON エクスポート。
  時刻はインスタンスのタイムゾーンで解釈されます。一覧では内容の要約を確認でき、メモを付けたり、整理の対象外と
  して固定したりできます。削除はまず台帳に記録されるため、クリーンアップなどごみ箱を通らずに削除された
  アーカイブの記録も、タグ・概要・読書進捗ごとごみ箱画面から復元できます（カテゴリとブックマークはスナップショットで戻します）。ライブラリの 4 分の 1 を超える整理は事前に確認を
  求めます。
- **リソースモニター**：設定画面のリアルタイムパネル。開いている間だけサンプリングし、閉じれば即座に停止します
  —— サービスとプラグインのプロセスの CPU / メモリ、cgroup の anon / file / slab とメモリ PSI、実際のプロセスツリー、
  ローカルモデルの利用状況、キューの段階別所要時間、Redis のカウンタを 1 画面に。各数値には観測窓と取得元が付き、
  読めない値は 0 ではなく `—` と表示されます。不具合報告のパッケージに同じスナップショット（`resources.json`、
  マスク済み）を含められ、debug ビルドではプロセス名に `(debug)` が付きます。
- **ネイティブパッケージ**：Linux（x86_64 / aarch64）、macOS（Apple Silicon）、FreeBSD。展開するだけで動き、
  コンテナは不要です。

## クイックスタート

```sh
docker run -d --name lanrurugi -p 3000:3000 \
  -v /path/to/manga:/home/koyomi/lanrurugi/content \
  -v /path/to/thumb:/home/koyomi/lanrurugi/thumb \
  -v /path/to/database:/home/koyomi/lanrurugi/database \
  ghcr.io/thelastfantasy/lanrurugi:beta
```

`http://<ホスト>:3000` を開きます。イメージに Redis が同梱されているため、別途データベースコンテナは不要です。
NAS の管理画面、リバースプロキシ、LANraragi からの移行を含む完全な compose は
[ドキュメント](https://docs.lrrg.dev/en/guide/install/) にあります。

<!-- `:latest` は正式版のリリース時にだけ付くため 1.0 より前には存在せず、上では当面 `:beta` を使っています。1.0 のリリース時に `:latest` に戻します（docs/dev/release.md 参照）。 -->
イメージは 3 チャンネルあります。`:latest` は正式版（1.0 のリリース後に利用可能）、`:beta` はプレリリース版、
`:edge` は `dev` ブランチの最新コードです（壊れることがあるため、本番のライブラリには向けないでください）。

> **管理者の初期パスワードは `kamimamita`**（LANraragi と同じ）です。初回ログイン後すぐに設定画面で変更し、
> 初期パスワードのままのインスタンスを LAN の外に公開しないでください。

## ドキュメント

- ユーザーガイド：<https://docs.lrrg.dev/en/>
- API リファレンス：<https://api.lrrg.dev/> — サーバーのルート定義から生成された OpenAPI
- プラグイン SDK：<https://sdk.lrrg.dev/>
- AI ツール向け：<https://lrrg.dev/llms.txt>

## コントリビュート

ツールチェーンの固定方法、ビルド・テスト手順、変更時に必ず通すチェックは [CONTRIBUTING.md](./CONTRIBUTING.md)
（中国語）を参照してください。設計ドキュメントは `specs/` 以下に機能ごとの `spec.md` / `plan.md` / `tasks.md`
として置かれています。

## 変更履歴

`v*` タグを打つと、コミット履歴から中英 2 言語のリリースノートが生成されます（アプリ内のアップグレード通知も
同じ内容）：[Releases](https://github.com/thelastfantasy/LANrurugi/releases)。

## ライセンス

MIT または Apache 2.0 のいずれかを選択できます — [LICENSE-MIT](./LICENSE-MIT) と [LICENSE-APACHE](./LICENSE-APACHE) を参照。
