//! 利用者に見せる文の表(PROTOCOL.md §4.6)。ホストの `args.locale`(無ければ `ja`)で引く。
//!
//! いまは日本語だけ。言語を足すときは `"en": { … }` のように表を足すだけでよい(無いキーは `ja` に落ちる)。

rasq_plugin_kit::i18n_catalog! {
    pub(crate) static TEXT = {
        "ja": {
            // sns.timelines
            "timeline.home" => "フォロー中",
            "timeline.global" => "Discover",
            // sns.actions
            "action.reply" => "返信",
            "action.repost" => "リポスト",
            "action.favorite" => "いいね",
            "action.quote" => "引用",
            // 設定ページ: 共通
            "settings.save" => "保存",
            "settings.onBskyApp" => "bsky.app で設定します",
            // account
            "account.title" => "アカウント",
            "account.profile" => "プロフィール",
            "account.displayName" => "表示名",
            "account.description" => "自己紹介",
            "account.avatarBanner" => "アイコン・バナー",
            "account.avatarBanner.sub" => "bsky.app のプロフィールの「プロフィールを編集」で変えます",
            "account.visibility" => "公開範囲",
            "account.noUnauthenticated" => "ログアウト中の人に見せない",
            "account.noUnauthenticated.sub" => "Bluesky のアプリと bsky.app で、ログインしていない人にプロフィールと投稿を見せないよう求めます",
            "account.allowIncoming" => "チャットを受け付ける相手",
            "account.allowIncoming.all" => "だれでも",
            "account.allowIncoming.following" => "フォローしている人だけ",
            "account.allowIncoming.none" => "受け付けない",
            "account.moderation" => "モデレーションと成人向けコンテンツ",
            "account.savedOnPds" => "これらの設定はあなたの Bluesky のリポジトリ(PDS)に保存されます。",
            // timeline
            "timeline.title" => "タイムライン",
            "timeline.shownPosts" => "フォロー中のタイムラインに出す投稿",
            "timeline.withReposts" => "リポストを表示",
            "timeline.withReposts.sub" => "オフにするとリポストを外します",
            "timeline.withReplies" => "返信を表示",
            "timeline.withReplies.sub" => "オフにすると、ほかの人への返信を外します(自分の投稿へのつながりは残します)",
            // safety
            "safety.title" => "プライバシーとセーフモード",
            "safety.sensitiveMedia" => "閲覧注意(ラベル付き)メディア",
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
            "security.loginWith.value" => "アプリパスワード",
            "security.appPasswordNote" => "アプリパスワードでは、パスワードの変更やアカウントの削除はできません。使わなくなったら bsky.app で削除(失効)できます。",
            "security.onBskyApp" => "bsky.app で行う設定",
            "security.appPasswords" => "アプリパスワードの管理",
            "security.appPasswords.sub" => "発行済みアプリパスワードの確認と削除",
            "security.account" => "メールアドレス・パスワード・二要素認証",
            "security.account.sub" => "パスワードが必要な操作は bsky.app で行います",
            // 設定の書き込みのエラー(利用者に出る)
            "error.tooLong" => "{max} 文字までです",
            "error.notBool" => "値は true / false です",
            "error.allowIncoming" => "all / following / none のどれかです",
            "error.emptyPost" => "本文か添付が要ります(Bluesky に投票はありません)",
        },
    };
}
