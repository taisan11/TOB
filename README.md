# TOB

Minecraft Bedrock Edition 最新BDSの NetherNet transport 向け Rust + Tokio 製Proxy/Plugin Serverです。

## Workspace

- `crates/cli` (`tob`): 設定の読込・検証、Pluginロード、Proxy起動、BDS起動管理のエントリポイント
- `crates/core`: BDS NetherNet signaling TCP のTLS透過中継
- `crates/plugin`: rustyscript + deno_core による TypeScript/JavaScript モジュール実行基盤
- `crates/protocol`: TOBが所有するBedrock/NetherNetプロトコル定義の置き場

`bedrock-rs` は依存していません。`tob setup` はMicrosoftの公式download manifestからBDSを取得し、`server.properties` を `transport=nethernet` に設定して `tob.toml` を生成します。`tob serve` はBDSを起動し、NetherNet signalingのTCPをTLSごとBDSへ透過中継します。signalで交換されるICE候補とUDPゲーム通信はBDSが直接処理します。

**この段階の制約:** クライアントのsignaling接続はTOBを経由しますが、ICE/DTLS/SCTPのゲーム通信はSDPで通知されるBDSへ直結します。これはまずBEクライアントが参加できる素の中継であり、ゲームパケットの中継・Plugin hookはまだありません。インターネット越しに使う場合はTCP `19132` に加え、BDSの `server-udp-ports` に設定したUDP `19140-19155` をBDSホストへ公開してください。

## 実行

```sh
cargo run -p tob-cli -- setup
cargo run -p tob-cli -- serve
```

公式BDSがサポートする実行環境はx86_64 Linux/Windowsです。macOSでは `setup --platform linux` で取得できますが、そのまま実行はできません。LinuxまたはWindowsホストで `tob serve` を実行してください。生成されたTOBアドレスとport (`19132`) をBedrockクライアントに登録します。

Pluginディレクトリには `.ts` または `.js` ファイルを置けます。現在はロード／評価のみで、ホストAPIやパケットイベント連携は未実装です。RustyscriptのV8を含むため初回ビルドには時間がかかる場合があります。

プロトコル実装の参照先:

- [Mojang Bedrock Protocol Docs](https://github.com/Mojang/bedrock-protocol-docs)
- [Bedrock Wiki — Servers](https://wiki.bedrock.dev/servers/bedrock)
- [Minecraft Wiki — Bedrock Edition protocol](https://minecraft.wiki/w/Bedrock_Edition_protocol)
