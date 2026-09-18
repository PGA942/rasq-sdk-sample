# {{name}} — SNS プラグインの雛形

`rasq init --sns {{id}}` で作られた SNS プラグイン。`sns.*`(rasq-sdk-sample `docs/sns-boundary.md`)に答える。
`manifest.json` は function.json と同じ形(`kind: sns`、`actions[].genreTag`)。
- wasm そのものの確認: `rasq call {{name}} <fn> --args '<JSON>'`(wasmi で動かす。`cargo test` は native なので、時刻 / 乱数 / スレッドを呼ぶ依存が wasm でだけ panic するのはここで見つかる — rasq-sdk-sample `docs/cli.md` §5 の落とし穴)
