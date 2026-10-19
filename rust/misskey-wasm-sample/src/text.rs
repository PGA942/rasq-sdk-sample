//! 利用者に見せる文の表(PROTOCOL.md §4.6)。ホストの `args.locale`(無ければ `ja`)で引く。
//!
//! いまは日本語だけ。言語を足すときは `"en": { … }` のように表を足すだけでよい(無いキーは `ja` に落ちる)。

rasq_plugin_kit::i18n_catalog! {
    pub(crate) static TEXT = {
        "ja": {
            // 画面コンストラクタのデモ(本体ツリー / FOW)
            "demo.fowButton" => "FOWデモ",
            "fow.title" => "リアクション (wasm)",
            "fow.close" => "閉じる",
            "fow.prompt" => "サンプルプラグインの FOW (postId: {postId})",
            "fow.count" => "タップ {n} 回",
            "fow.confirm.title" => "閉じますか?",
            "fow.confirm.message" => "リアクションを選ばずに閉じますか?",
            "fow.confirm.discard" => "破棄",
            "fow.confirm.keep" => "続ける",
            // sns.actions / タイルのアクションバー
            "action.reply" => "返信",
            "action.renote" => "リノート",
            "action.reaction" => "リアクション",
            // sns.timelines
            "timeline.home" => "ホーム",
            "timeline.social" => "ソーシャル",
            "timeline.local" => "ローカル",
            "timeline.global" => "グローバル",
            "timeline.featured" => "Today",
            // TL プラグインのデモ(tl.timelines / tl.customize)
            "tl.reactionRank" => "リアクション順",
            "tl.first" => "👑 リアクション1位 ({n}件)",
            "tl.second" => "リアクション2位 ({n}件)",
            // 設定ページ: 共通
            "settings.save" => "保存",
            "settings.webOnly" => "web 版で編集します",
            // account
            "account.title" => "アカウント",
            "account.profile" => "プロフィール",
            "account.name" => "名前",
            "account.description" => "自己紹介",
            "account.location" => "場所",
            "account.birthday" => "誕生日(例: 2000-01-31)",
            "account.profileOnWeb" => "アイコン・バナー・補足情報",
            "account.visibility" => "公開範囲",
            "account.isLocked" => "フォローを承認制にする",
            "account.isLocked.sub" => "フォローされる前にあなたの承認が必要になります",
            "account.autoAcceptFollowed" => "フォロー中の人からのフォローは自動で承認",
            "account.isExplorable" => "アカウントを見つけやすくする",
            "account.isExplorable.sub" => "「みつける」やユーザー検索に表示されます",
            "account.hideOnlineStatus" => "オンライン状態を隠す",
            "account.publicReactions" => "リアクション一覧を公開する",
            "account.noCrawle" => "検索エンジンに載せない",
            "account.noCrawle.sub" => "検索エンジンによるインデックスを避けるよう求めます",
            "account.followingVisibility" => "フォローの一覧を見せる相手",
            "account.followersVisibility" => "フォロワーの一覧を見せる相手",
            "account.visibility.public" => "公開",
            "account.visibility.followers" => "フォロワーだけ",
            "account.visibility.private" => "非公開",
            "account.kind" => "アカウントの種類",
            "account.isBot" => "Botとして設定",
            "account.isCat" => "Catにする",
            "account.isCat.sub" => "発言が Cat 語になり、猫耳が付きます",
            "account.savedOnServer" => "これらの設定は接続先の Misskey サーバーに保存されます。",
            // timeline
            "timeline.title" => "タイムライン",
            "timeline.shownPosts" => "表示する投稿",
            "timeline.withRenotes" => "リノートを表示",
            "timeline.withRenotes.sub" => "オフにすると引用のないリノートを取得しません",
            "timeline.withReplies" => "他の人への返信を表示",
            "timeline.withReplies.sub" => "フォロー中の人が他の人へ送った返信をTLに含めます",
            // safety
            "safety.title" => "プライバシーとセーフモード",
            "safety.sensitiveMedia" => "閲覧注意(センシティブ)メディア",
            "safety.blur" => "ぼかして表示",
            "safety.blur.sub" => "タップで開きます(標準)",
            "safety.show" => "ぼかさず表示",
            "safety.hide" => "表示しない(セーフモード)",
            "safety.hide.sub" => "閲覧注意メディアをタイルから取り除きます",
            "safety.cw" => "注釈(CW)",
            "safety.openCw" => "注釈付き投稿を自動で開く",
            // security
            "security.title" => "セキュリティ",
            "security.onWeb" => "web で行う設定",
            "security.password" => "パスワード・二要素認証・ログイン履歴",
            "security.password.sub" => "パスワードが必要な操作は web 版で行います",
            "security.tokens" => "アクセストークンの管理",
            "security.tokens.sub" => "発行済みトークンの確認と失効",
            "security.apps" => "連携しているアプリ",
            "security.apps.sub" => "アプリに与えた許可の確認と取り消し",
            "security.tokenNote" => "アクセストークンは端末内の安全な領域にのみ保存されます。",
            // 設定の書き込みのエラー(利用者に出る)
            "error.notBool" => "値は true / false です",
            "error.birthday" => "誕生日は 2000-01-31 の形で入れてください",
            "error.visibility" => "公開範囲は public / followers / private です",
        },
    };
}
