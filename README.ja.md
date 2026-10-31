[简体中文](./README.md) | [English](./README.en.md) | 日本語

# LANrurugi

[![CI](https://github.com/thelastfantasy/LANrurugi/actions/workflows/ci.yml/badge.svg?branch=dev)](https://github.com/thelastfantasy/LANrurugi/actions/workflows/ci.yml)
[![CodeQL](https://github.com/thelastfantasy/LANrurugi/actions/workflows/codeql.yml/badge.svg?branch=dev)](https://github.com/thelastfantasy/LANrurugi/actions/workflows/codeql.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](./LICENSE)

セルフホスト型の漫画・同人誌ライブラリ管理ツール。Rust バックエンド + React フロントエンドで、
[LANraragi](https://github.com/Difegue/LANraragi) をゼロから書き直したものです。

## LANraragi との関係

ライブラリのディレクトリ構成、タグ構文、HTTP API は LANraragi をそのまま踏襲しているため、既存のサードパーティ
製クライアント・リーダー・移行手順はそのまま使えます。乗り換えは同じ漫画ディレクトリをマウントし、バックアップ
JSON を読み込むだけです。書き直した理由は速度と堅牢性で、バックエンドを Rust + Tokio にしてサムネイル・OCR・
画像比較といった CPU 依存の処理が本当に全コアを使えるようにし、ダウンロードをプラグインのサンドボックスから
ホストプロセスへ移したことで、実際のバイト単位の進捗とドメイン別のレート制限が可能になりました。機能面は
「まず同等」を優先し、購読による自動追従、共有・取り込み、ページ内翻訳、ネイティブパッケージは同等性の上に
足したものです。項目ごとの比較（意図的に残した差異を含む）は
[docs/comparison-with-lanraragi.md](./docs/comparison-with-lanraragi.md)（中国語）を参照してください。

## できること

- **ライブラリと閲覧**：カテゴリ、タグ、読書進捗、全文検索とタグ絞り込み。本棚 / グリッド / リスト表示、
  UI は 12 言語。
- **プラグイン**：メタデータ・ダウンロード・ログインの 3 種類が Deno サブプロセスで動作し、名前空間ごとに権限を
  分離。AI ウィザードでサイト向けのプラグインを生成できます。
- **ダウンロード**：バイト単位の進捗、ドメイン別の同時実行数とレート制限。新刊を検出してルールで判定し、
  プレビュー後にキューへ入れる購読機能。
- **共有**：アーカイブや単行本を他の人へ共有し、相手はブラウザで読む・ダウンロードする・自分のインスタンスへ
  取り込む、のいずれかを選べます（直接接続・ホールパンチング・リレー。第三者サービス不要）。
- **ページ内翻訳**：OCR → 原文の消去 → 翻訳の組版。クラウド API でもローカルモデルでもよく、フォントは巻単位で
  キャッシュします。
- **電子書籍**：PDF、EPUB、MOBI/AZW3、DjVu、FB2、TXT をアーカイブと同じように登録・閲覧できます。
- **ネイティブパッケージ**：Linux（x86_64 / aarch64）、macOS（Apple Silicon）、FreeBSD。展開するだけで動き、
  コンテナは不要です。

## クイックスタート

```sh
docker run -d --name lanrurugi -p 3000:3000 \
  -v /path/to/manga:/home/koyomi/lanrurugi/content \
  -v /path/to/thumb:/home/koyomi/lanrurugi/thumb \
  -v /path/to/database:/home/koyomi/lanrurugi/database \
  ghcr.io/thelastfantasy/lanrurugi:latest
```

`http://<ホスト>:3000` を開きます。イメージに Redis が同梱されているため、別途データベースコンテナは不要です。
NAS の管理画面、リバースプロキシ、LANraragi からの移行を含む完全な compose は
[ドキュメント](https://docs.lrrg.dev/en/guide/install/) にあります。

イメージは 2 チャンネルあります。`:latest` はリリース版、`:edge` は `dev` ブランチの最新コードです
（壊れることがあるため、本番のライブラリには向けないでください）。

> **管理者の初期パスワードは `kamimamita`**（LANraragi と同じ）です。初回ログイン後すぐに設定画面で変更し、
> 初期パスワードのままのインスタンスを LAN の外に公開しないでください。

## ドキュメント

- ユーザーガイド：<https://docs.lrrg.dev/en/>
- API リファレンス：<https://api.lrrg.dev/> — サーバーのルート定義から生成された OpenAPI（手書きではありません）
- プラグイン SDK：<https://sdk.lrrg.dev/>
- AI ツール向け：<https://lrrg.dev/llms.txt>

## コントリビュート

ツールチェーンの固定方法、ビルド・テスト手順、変更時に必ず通すチェックは [CONTRIBUTING.md](./CONTRIBUTING.md)
（中国語）を参照してください。設計ドキュメントは `specs/` 以下に機能ごとの `spec.md` / `plan.md` / `tasks.md`
として置かれています。

## 変更履歴

`v*` タグを打つたびに、コミット履歴から中英 2 言語のリリースノートが生成されます —
[Releases](https://github.com/thelastfantasy/LANrurugi/releases) を参照してください。アプリ内のアップグレード
通知も同じ内容を読みます。

## ライセンス

MIT — [LICENSE](./LICENSE) を参照。
