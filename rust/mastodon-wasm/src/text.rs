//! 利用者に見せる文の表(PROTOCOL.md §4.6)。ホストの `args.locale`(無ければ `ja`)で引く。
//!
//! いまは日本語だけ。言語を足すときは `"en": { … }` のように表を足すだけでよい(無いキーは `ja` に落ちる)。

rasq_plugin_kit::i18n_catalog! {
    pub(crate) static TEXT = {
        "ja": {
            // sns.timelines
            "timeline.home" => "ホーム",
            "timeline.local" => "ローカル",
            "timeline.global" => "連合",
            "timeline.featured" => "トレンド",
            // sns.actions
            "action.reply" => "返信",
            "action.reblog" => "ブースト",
            "action.favourite" => "お気に入り",
            "action.bookmark" => "ブックマーク",
            // 設定ページ: 共通
            "settings.save" => "保存",
            "settings.webOnly" => "web 版で編集します",
            // account
            "account.title" => "アカウント",
            "account.profile" => "プロフィール",
            "account.displayName" => "表示名",
            "account.note" => "自己紹介",
            "account.profileOnWeb" => "アイコン・ヘッダー・補足情報",
            "account.visibility" => "公開範囲",
            "account.locked" => "フォローを承認制にする",
            "account.locked.sub" => "フォローされる前にあなたの承認が必要になります",
            "account.discoverable" => "おすすめやディレクトリに載せる",
            "account.discoverable.sub" => "ほかの人にアカウントをすすめてもよいか",
            "account.indexable" => "公開の投稿を検索できるようにする",
            "account.hideCollections" => "フォローとフォロワーの一覧を隠す",
            "account.defaultPrivacy" => "投稿の既定の公開範囲",
            "account.privacy.public" => "公開",
            "account.privacy.public.sub" => "だれでも見られ、公開タイムラインにも出ます",
            "account.privacy.unlisted" => "未収載(ひかえめな公開)",
            "account.privacy.unlisted.sub" => "だれでも見られますが、公開タイムラインには出ません",
            "account.privacy.private" => "フォロワーのみ",
            "account.defaultSensitive" => "メディアを常に閲覧注意にする",
            "account.defaultSensitive.sub" => "投稿する画像や動画に、既定で閲覧注意を付けます",
            "account.kind" => "アカウントの種類",
            "account.bot" => "Botとして設定",
            "account.bot.sub" => "自動で投稿するアカウントであることを示します",
            "account.savedOnServer" => "これらの設定は接続先の Mastodon サーバーに保存されます。",
            // timeline
            "timeline.title" => "タイムライン",
            "timeline.shownPosts" => "表示する投稿",
            "timeline.withReposts" => "ブーストを表示",
            "timeline.withReposts.sub" => "オフにするとブーストをタイムラインから外します",
            "timeline.withReplies" => "返信を表示",
            "timeline.withReplies.sub" => "オフにすると、ほかの人への返信をタイムラインから外します(自分の投稿へのつながりは残します)",
            // safety
            "safety.title" => "プライバシーとセーフモード",
            "safety.sensitiveMedia" => "閲覧注意(センシティブ)メディア",
            "safety.blur" => "ぼかして表示",
            "safety.blur.sub" => "タップで開きます(標準)",
            "safety.show" => "ぼかさず表示",
            "safety.hide" => "表示しない(セーフモード)",
            "safety.hide.sub" => "閲覧注意メディアをタイルから取り除きます",
            "safety.cw" => "コンテンツ警告(CW)",
            "safety.openCw" => "CW 付きの投稿を自動で開く",
            // security
            "security.title" => "セキュリティ",
            "security.scopes" => "このアプリに許可した権限",
            "security.scopes.value" => "読み取り・書き込み・フォロー",
            "security.onWeb" => "web で行う設定",
            "security.password" => "メールアドレス・パスワード・ログイン中の端末",
            "security.password.sub" => "パスワードが必要な操作は web 版で行います",
            "security.twoFactor" => "二要素認証",
            "security.apps" => "連携しているアプリ",
            "security.apps.sub" => "アプリに与えた許可の確認と取り消し",
            "security.tokenNote" => "アクセストークンは端末内の安全な領域にのみ保存されます。",
            // 設定の書き込みのエラー(利用者に出る)
            "error.notBool" => "値は true / false です",
            "error.privacy" => "公開範囲は public / unlisted / private です",
        },
    };
}
