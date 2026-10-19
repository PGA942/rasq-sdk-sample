//! 利用者に見せる文の表(PROTOCOL.md §4.6)。ホストの `args.locale`(無ければ `ja`)で引く。
//!
//! いまは日本語だけ。言語を足すときは `"en": { … }` のように表を足すだけでよい(無いキーは `ja` に落ちる)。

rasq_plugin_kit::i18n_catalog! {
    pub(crate) static TEXT = {
        "ja": {
            // sns.timelines / sns.actions
            "timeline.home" => "ホーム",
            "timeline.local" => "みんな",
            "action.reply" => "返信",
            "action.repost" => "リポスト",
            "action.favorite" => "いいね",
            "action.quote" => "引用",
            // エラー(利用者に出る)
            "error.postTooLong" => "本文は {max} 文字まで",
            "error.loginOauthOnly" => "mixi2 は OAuth でログインします(RASQ サーバーのブローカー経由)。動作確認はホスト「dummy」で。",
            "error.accountReadOnly" => "mixi2 の API ではアカウントの設定を変えられません",
            // 設定ページ: account
            "account.title" => "アカウント",
            "account.profile" => "プロフィール",
            "account.displayName" => "表示名",
            "account.handle" => "ユーザー名",
            "account.bio" => "自己紹介",
            "account.readOnlyNotice" => "mixi2 の API ではプロフィールや公開範囲を変えられないため、ここでは表示だけです。変えるときは mixi2 のアプリで行ってください。",
            // timeline
            "timeline.title" => "タイムライン",
            "timeline.shownPosts" => "表示する投稿",
            "timeline.withReposts" => "リポストを表示",
            "timeline.withReposts.sub" => "オフにするとリポストをタイムラインから外します",
            "timeline.withReplies" => "返信を表示",
            "timeline.withReplies.sub" => "オフにすると返信をタイムラインから外します",
            // safety
            "safety.title" => "プライバシーとセーフモード",
            "safety.sensitiveMedia" => "閲覧注意(センシティブ)メディア",
            "safety.blur" => "ぼかして表示",
            "safety.blur.sub" => "タップで開きます(標準)",
            "safety.show" => "ぼかさず表示",
            "safety.hide" => "表示しない(セーフモード)",
            "safety.hide.sub" => "閲覧注意メディアをタイルから取り除きます",
            "safety.cw" => "注釈",
            "safety.openCw" => "注釈付き投稿を自動で開く",
            // security
            "security.title" => "セキュリティ",
            "security.loginMethod" => "ログインの方法",
            "security.loginWith" => "ログインに使っているもの",
            "security.loginWith.value" => "RASQ サーバー経由の OAuth",
            "security.tokenNote" => "RASQ が受け取るのは mixi2 が発行したアクセストークンだけで、パスワードは受け取りません。トークンは端末内の安全な領域にのみ保存されます。",
            "security.elsewhere" => "パスワード・二要素認証・連携しているアプリの取り消しは、mixi2 のアプリの設定で行ってください。",
        },
    };
}
