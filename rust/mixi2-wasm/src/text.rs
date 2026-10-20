//! 利用者に見せる文の表(PROTOCOL.md §4.6)。ホストの `args.locale`(無ければ `ja`)で引く。
//!
//! 基準は `ja`。`en` / `zh-Hans` / `zh-Hant` / `ko` は同じキーを全部持つ(テストで確かめる)。言語を足すときは表を足すだけ。

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
            "error.emptyPost" => "本文を入れてください(mixi2 には添付 / 投票を送れません)",
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
        "en": {
            "timeline.home" => "Home",
            "timeline.local" => "Everyone",
            "action.reply" => "Reply",
            "action.repost" => "Repost",
            "action.favorite" => "Like",
            "action.quote" => "Quote",
            "error.postTooLong" => "Text can be up to {max} characters",
            "error.emptyPost" => "Enter some text (attachments and polls can't be sent to mixi2)",
            "error.loginOauthOnly" => "mixi2 signs in with OAuth (through the RASQ server's broker). For testing, use the host “dummy”.",
            "error.accountReadOnly" => "Account settings can't be changed through the mixi2 API",
            "account.title" => "Account",
            "account.profile" => "Profile",
            "account.displayName" => "Display name",
            "account.handle" => "Username",
            "account.bio" => "Bio",
            "account.readOnlyNotice" => "The mixi2 API can't change your profile or visibility, so they're only shown here. To change them, use the mixi2 app.",
            "timeline.title" => "Timeline",
            "timeline.shownPosts" => "Posts to show",
            "timeline.withReposts" => "Show reposts",
            "timeline.withReposts.sub" => "Turn off to remove reposts from your timeline",
            "timeline.withReplies" => "Show replies",
            "timeline.withReplies.sub" => "Turn off to remove replies from your timeline",
            "safety.title" => "Privacy and safe mode",
            "safety.sensitiveMedia" => "Sensitive media",
            "safety.blur" => "Blur",
            "safety.blur.sub" => "Tap to reveal (default)",
            "safety.show" => "Show without blur",
            "safety.hide" => "Hide (safe mode)",
            "safety.hide.sub" => "Removes sensitive media from posts",
            "safety.cw" => "Content warnings",
            "safety.openCw" => "Automatically expand posts with content warnings",
            "security.title" => "Security",
            "security.loginMethod" => "Sign-in method",
            "security.loginWith" => "Signed in with",
            "security.loginWith.value" => "OAuth via the RASQ server",
            "security.tokenNote" => "RASQ only receives the access token issued by mixi2, never your password. The token is stored only in this device's secure storage.",
            "security.elsewhere" => "Change your password, two-factor authentication and connected apps in the mixi2 app's settings.",
        },
        "zh-Hans": {
            "timeline.home" => "首页",
            "timeline.local" => "大家",
            "action.reply" => "回复",
            "action.repost" => "转发",
            "action.favorite" => "喜欢",
            "action.quote" => "引用",
            "error.postTooLong" => "正文最多 {max} 个字符",
            "error.emptyPost" => "请输入正文（无法向 mixi2 发送附件或投票）",
            "error.loginOauthOnly" => "mixi2 通过 OAuth 登录（经由 RASQ 服务器的中转）。测试时请使用主机“dummy”。",
            "error.accountReadOnly" => "无法通过 mixi2 的 API 更改账号设置",
            "account.title" => "账号",
            "account.profile" => "个人资料",
            "account.displayName" => "昵称",
            "account.handle" => "用户名",
            "account.bio" => "个人简介",
            "account.readOnlyNotice" => "mixi2 的 API 无法更改个人资料和可见范围，因此这里仅供查看。如需更改，请使用 mixi2 应用。",
            "timeline.title" => "时间线",
            "timeline.shownPosts" => "显示的帖子",
            "timeline.withReposts" => "显示转发",
            "timeline.withReposts.sub" => "关闭后，时间线中将不显示转发",
            "timeline.withReplies" => "显示回复",
            "timeline.withReplies.sub" => "关闭后，时间线中将不显示回复",
            "safety.title" => "隐私与安全模式",
            "safety.sensitiveMedia" => "敏感媒体",
            "safety.blur" => "模糊显示",
            "safety.blur.sub" => "点按即可查看（默认）",
            "safety.show" => "直接显示",
            "safety.hide" => "不显示（安全模式）",
            "safety.hide.sub" => "从帖子卡片中移除敏感媒体",
            "safety.cw" => "内容警告",
            "safety.openCw" => "自动展开带有内容警告的帖子",
            "security.title" => "安全",
            "security.loginMethod" => "登录方式",
            "security.loginWith" => "当前登录方式",
            "security.loginWith.value" => "经由 RASQ 服务器的 OAuth",
            "security.tokenNote" => "RASQ 只会收到 mixi2 签发的访问令牌，不会收到你的密码。令牌仅保存在本设备的安全存储区中。",
            "security.elsewhere" => "密码、双重验证以及撤销已关联的应用，请在 mixi2 应用的设置中进行。",
        },
        "zh-Hant": {
            "timeline.home" => "首頁",
            "timeline.local" => "大家",
            "action.reply" => "回覆",
            "action.repost" => "轉發",
            "action.favorite" => "喜歡",
            "action.quote" => "引用",
            "error.postTooLong" => "內文最多 {max} 個字元",
            "error.emptyPost" => "請輸入內文（無法向 mixi2 傳送附件或投票）",
            "error.loginOauthOnly" => "mixi2 透過 OAuth 登入（經由 RASQ 伺服器的中介）。測試時請使用主機「dummy」。",
            "error.accountReadOnly" => "無法透過 mixi2 的 API 變更帳號設定",
            "account.title" => "帳號",
            "account.profile" => "個人檔案",
            "account.displayName" => "顯示名稱",
            "account.handle" => "使用者名稱",
            "account.bio" => "自我介紹",
            "account.readOnlyNotice" => "mixi2 的 API 無法變更個人檔案與公開範圍，因此這裡僅供檢視。如需變更，請使用 mixi2 App。",
            "timeline.title" => "時間軸",
            "timeline.shownPosts" => "顯示的貼文",
            "timeline.withReposts" => "顯示轉發",
            "timeline.withReposts.sub" => "關閉後，時間軸將不顯示轉發",
            "timeline.withReplies" => "顯示回覆",
            "timeline.withReplies.sub" => "關閉後，時間軸將不顯示回覆",
            "safety.title" => "隱私與安全模式",
            "safety.sensitiveMedia" => "敏感媒體",
            "safety.blur" => "模糊顯示",
            "safety.blur.sub" => "點一下即可查看（預設）",
            "safety.show" => "直接顯示",
            "safety.hide" => "不顯示（安全模式）",
            "safety.hide.sub" => "從貼文卡片中移除敏感媒體",
            "safety.cw" => "內容警告",
            "safety.openCw" => "自動展開含有內容警告的貼文",
            "security.title" => "安全性",
            "security.loginMethod" => "登入方式",
            "security.loginWith" => "目前的登入方式",
            "security.loginWith.value" => "經由 RASQ 伺服器的 OAuth",
            "security.tokenNote" => "RASQ 只會收到 mixi2 發行的存取權杖，不會收到您的密碼。權杖僅儲存在本裝置的安全儲存區中。",
            "security.elsewhere" => "密碼、雙重驗證以及撤銷已連結的應用程式，請在 mixi2 App 的設定中進行。",
        },
        "ko": {
            "timeline.home" => "홈",
            "timeline.local" => "모두",
            "action.reply" => "답글",
            "action.repost" => "리포스트",
            "action.favorite" => "좋아요",
            "action.quote" => "인용",
            "error.postTooLong" => "본문은 최대 {max}자입니다",
            "error.emptyPost" => "본문을 입력하세요(mixi2에는 첨부 파일이나 투표를 보낼 수 없습니다)",
            "error.loginOauthOnly" => "mixi2는 OAuth로 로그인합니다(RASQ 서버의 브로커 경유). 동작 확인은 호스트 'dummy'로 하세요.",
            "error.accountReadOnly" => "mixi2 API로는 계정 설정을 바꿀 수 없습니다",
            "account.title" => "계정",
            "account.profile" => "프로필",
            "account.displayName" => "표시 이름",
            "account.handle" => "사용자 이름",
            "account.bio" => "자기소개",
            "account.readOnlyNotice" => "mixi2 API로는 프로필이나 공개 범위를 바꿀 수 없어 여기서는 표시만 합니다. 바꾸려면 mixi2 앱에서 하세요.",
            "timeline.title" => "타임라인",
            "timeline.shownPosts" => "표시할 게시물",
            "timeline.withReposts" => "리포스트 표시",
            "timeline.withReposts.sub" => "끄면 타임라인에서 리포스트를 제외합니다",
            "timeline.withReplies" => "답글 표시",
            "timeline.withReplies.sub" => "끄면 타임라인에서 답글을 제외합니다",
            "safety.title" => "개인정보 보호 및 세이프 모드",
            "safety.sensitiveMedia" => "민감한 미디어",
            "safety.blur" => "흐리게 표시",
            "safety.blur.sub" => "탭하면 볼 수 있습니다(기본)",
            "safety.show" => "흐리게 하지 않고 표시",
            "safety.hide" => "표시하지 않음(세이프 모드)",
            "safety.hide.sub" => "게시물 타일에서 민감한 미디어를 제거합니다",
            "safety.cw" => "콘텐츠 경고",
            "safety.openCw" => "콘텐츠 경고가 있는 게시물 자동으로 펼치기",
            "security.title" => "보안",
            "security.loginMethod" => "로그인 방법",
            "security.loginWith" => "로그인에 사용 중인 수단",
            "security.loginWith.value" => "RASQ 서버를 경유한 OAuth",
            "security.tokenNote" => "RASQ가 받는 것은 mixi2가 발급한 액세스 토큰뿐이며 비밀번호는 받지 않습니다. 토큰은 기기의 안전한 영역에만 저장됩니다.",
            "security.elsewhere" => "비밀번호, 2단계 인증, 연동된 앱 취소는 mixi2 앱의 설정에서 하세요.",
        },
    };
}

#[cfg(test)]
mod tests {
    use super::TEXT;
    use std::collections::BTreeSet;

    /// この表の言語ごとのキー(`Catalog::check` は「基準に無いキー」と差し込みの食い違いしか見ないので、訳の抜けはここで見る)。
    fn table_keys() -> Vec<(String, BTreeSet<String>)> {
        let src = include_str!("text.rs");
        let body = src.split("i18n_catalog! {").nth(1).and_then(|s| s.split("\n}").next()).expect("i18n_catalog!");
        let mut out: Vec<(String, BTreeSet<String>)> = Vec::new();
        for line in body.lines().map(str::trim) {
            if let Some(tag) = line.strip_prefix('"').and_then(|l| l.strip_suffix("\": {")) {
                out.push((tag.to_string(), BTreeSet::new()));
            } else if let Some((key, _)) = line.strip_prefix('"').and_then(|l| l.split_once("\" =>")) {
                out.last_mut().expect("表の外のキー").1.insert(key.to_string());
            }
        }
        out
    }

    #[test]
    fn every_locale_has_every_key() {
        assert!(TEXT.check().is_empty(), "{:?}", TEXT.check());
        let tables = table_keys();
        let tags: Vec<&str> = tables.iter().map(|(t, _)| t.as_str()).collect();
        assert_eq!(tags, TEXT.locales());
        assert_eq!(tags, ["ja", "en", "zh-Hans", "zh-Hant", "ko"]);
        for (tag, keys) in &tables[1..] {
            let missing: Vec<_> = tables[0].1.difference(keys).collect();
            assert!(missing.is_empty(), "{tag} に訳が無いキー: {missing:?}");
        }
    }
}
