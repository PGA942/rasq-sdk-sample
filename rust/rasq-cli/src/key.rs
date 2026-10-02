//! 開発者鍵(docs/cli.md §6、D-244d): `~/.config/rasq/dev-key.toml`(0600)の ed25519。
//! 署名対象は `.rasqpkg` 全体の SHA-512 ダイジェスト(生 64 バイト)= rasq-client の `verifyArtifact` と同じ。
//! ストア鍵は Central が持つ(D-034)ので、これはサイドロード(開発者鍵 + 警告)用。

use anyhow::{anyhow, bail, Context, Result};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

#[derive(Debug, Serialize, Deserialize)]
pub struct DevKey {
    pub alg: String,
    /// 32 バイトの秘密シード(base64)。
    pub secret: String,
    /// 32 バイトの公開鍵(base64)。ホストの `storePublicKey` と同じ形。
    pub public: String,
    pub created_at: String,
}

pub fn path() -> Result<PathBuf> {
    if let Some(p) = std::env::var_os("RASQ_DEV_KEY") {
        return Ok(PathBuf::from(p));
    }
    Ok(crate::credentials::path()?.parent().unwrap().join("dev-key.toml"))
}

pub fn load() -> Result<Option<(DevKey, SigningKey)>> {
    let p = path()?;
    if !p.is_file() {
        return Ok(None);
    }
    let k: DevKey = toml::from_str(&std::fs::read_to_string(&p)?).with_context(|| format!("{} の形が違います", p.display()))?;
    if k.alg != "ed25519" {
        bail!("{}: alg {} には対応していません", p.display(), k.alg);
    }
    let seed: [u8; 32] = B64.decode(&k.secret).context("secret が base64 ではありません")?.try_into().map_err(|_| anyhow!("secret が 32 バイトではありません"))?;
    let sk = SigningKey::from_bytes(&seed);
    if B64.encode(sk.verifying_key().as_bytes()) != k.public {
        bail!("{}: public が secret と合いません", p.display());
    }
    Ok(Some((k, sk)))
}

pub fn init(force: bool) -> Result<DevKey> {
    let p = path()?;
    if p.exists() && !force {
        bail!("{} が既にあります(作り直すなら --force。古い鍵で署名した配布物は検証できなくなります)", p.display());
    }
    let mut seed = [0u8; 32];
    getrandom::getrandom(&mut seed).map_err(|e| anyhow!("乱数を取れません: {e}"))?;
    let sk = SigningKey::from_bytes(&seed);
    let k = DevKey { alg: "ed25519".into(), secret: B64.encode(seed), public: B64.encode(sk.verifying_key().as_bytes()), created_at: crate::credentials::now() };
    // シードは 0600 で作った一時ファイルに書いて置き換える(既定の権限で書いてから chmod しない、#5)。
    crate::private_file::write(&p, toml::to_string_pretty(&k)?.as_bytes())?;
    Ok(k)
}

/// 公開鍵の短い指紋(SHA-512 の先頭 8 バイト、hex)。
pub fn fingerprint(public_b64: &str) -> String {
    use sha2::Digest;
    let d = sha2::Sha512::digest(B64.decode(public_b64).unwrap_or_default());
    d[..8].iter().map(|b| format!("{b:02x}")).collect()
}

/// `.rasqpkg.sig.json` の中身。`digest` は配布物バイト列の SHA-512。
pub fn sign(sk: &SigningKey, digest: &[u8; 64]) -> Value {
    let sig = sk.sign(digest);
    json!({
        "alg": "ed25519",
        "publicKey": B64.encode(sk.verifying_key().as_bytes()),
        "signature": B64.encode(sig.to_bytes()),
        "digest": format!("sha512:{}", hex(digest)),
    })
}

/// `.sig.json` を配布物のダイジェストで確かめる。戻りは公開鍵(base64)。
pub fn verify(sig_json: &Value, digest: &[u8; 64]) -> Result<String> {
    if sig_json["alg"] != "ed25519" {
        bail!("alg が ed25519 ではありません: {}", sig_json["alg"]);
    }
    let expect = format!("sha512:{}", hex(digest));
    if sig_json["digest"].as_str() != Some(expect.as_str()) {
        bail!("sig.json の digest が配布物と違います(配布物が差し替わっている)");
    }
    let pk_b64 = sig_json["publicKey"].as_str().ok_or_else(|| anyhow!("publicKey がありません"))?;
    let pk: [u8; 32] = B64.decode(pk_b64)?.try_into().map_err(|_| anyhow!("publicKey が 32 バイトではありません"))?;
    let sig: [u8; 64] = B64.decode(sig_json["signature"].as_str().unwrap_or(""))?.try_into().map_err(|_| anyhow!("signature が 64 バイトではありません"))?;
    VerifyingKey::from_bytes(&pk)?.verify(digest, &Signature::from_bytes(&sig)).map_err(|_| anyhow!("署名が合いません"))?;
    Ok(pk_b64.to_string())
}

/// 鍵の操作の承認(Central `src/store/devkeys.rs` と同じ形、セキュリティ確認 2026-10-02 #18)。
/// 署名する文は `rasq.devkeys.v1|<action>|<対象の鍵の指紋>|<期限 (unix 秒)>`。トークンは
/// `{action, fingerprint, exp, publicKey, signature}` の JSON を URL-safe base64(パディング無し)にしたもの。
pub fn approve(sk: &SigningKey, action: &str, target: &str, ttl: u64) -> Result<String> {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let fingerprint = match action {
        "add" => {
            let raw = B64.decode(target.trim()).context("公開鍵が base64 ではありません")?;
            let pk: [u8; 32] = raw.try_into().map_err(|_| anyhow!("公開鍵が 32 バイトではありません"))?;
            VerifyingKey::from_bytes(&pk).map_err(|_| anyhow!("ed25519 の公開鍵ではありません"))?;
            fingerprint(target.trim())
        }
        "revoke" => {
            let t = target.trim().to_ascii_lowercase();
            if t.len() != 16 || !t.bytes().all(|b| b.is_ascii_hexdigit()) {
                bail!("指紋は 16 桁の 16 進です(コンソールの鍵の一覧に出ています)");
            }
            t
        }
        other => bail!("action は add か revoke です: {other}"),
    };
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_secs();
    let exp = now + ttl.clamp(60, 3600);
    let msg = format!("rasq.devkeys.v1|{action}|{fingerprint}|{exp}");
    let sig = sk.sign(msg.as_bytes());
    let doc = json!({
        "action": action,
        "fingerprint": fingerprint,
        "exp": exp,
        "publicKey": B64.encode(sk.verifying_key().as_bytes()),
        "signature": B64.encode(sig.to_bytes()),
    });
    Ok(URL_SAFE_NO_PAD.encode(doc.to_string()))
}

pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

pub fn sig_path(pkg: &Path) -> PathBuf {
    let mut s = pkg.as_os_str().to_owned();
    s.push(".sig.json");
    PathBuf::from(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;

    #[test]
    fn approvals_sign_the_central_message() {
        // Central(src/store/devkeys.rs)が確かめる文と同じものに署名する。
        let sk = SigningKey::from_bytes(&[9u8; 32]);
        let newkey = B64.encode(SigningKey::from_bytes(&[3u8; 32]).verifying_key().as_bytes());
        let t = approve(&sk, "add", &newkey, 600).unwrap();
        let v: Value = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(&t).unwrap()).unwrap();
        assert_eq!(v["action"], "add");
        assert_eq!(v["fingerprint"], fingerprint(&newkey));
        let msg = format!("rasq.devkeys.v1|add|{}|{}", fingerprint(&newkey), v["exp"]);
        let sig: [u8; 64] = B64.decode(v["signature"].as_str().unwrap()).unwrap().try_into().unwrap();
        sk.verifying_key().verify(msg.as_bytes(), &Signature::from_bytes(&sig)).unwrap();
        assert!(approve(&sk, "revoke", "nothex", 600).is_err());
        assert!(approve(&sk, "add", "AAAA", 600).is_err());
        assert!(approve(&sk, "rotate", "0123456789abcdef", 600).is_err());
        assert!(approve(&sk, "revoke", "0123456789ABCDEF", 600).is_ok());
    }
}
