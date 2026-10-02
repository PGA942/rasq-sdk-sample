//! 秘密(Access Token・開発者鍵のシード)を書くファイル(セキュリティ確認 2026-10-02 #5)。
//!
//! 以前は既定の権限で書いてから chmod していたので、緩い umask(022)だと、書いてから chmod するまでのあいだに
//! ほかのローカルユーザーがファイルを開いて(開いた記述子は chmod のあとも読める)中身を読めた。いまは:
//!
//! 1. 親ディレクトリを **0700** で作る(あれば 0700 にする)。
//! 2. 同じディレクトリに一時ファイルを `create_new` + **0600** で開く(作った瞬間から 0600。umask は権限を減らすだけ)。
//! 3. 中身を書いて `sync_all` し、`rename` で置き換える(読み手には古い中身か新しい中身のどちらかしか見えない)。
//! 4. 途中で失敗したら一時ファイルを消す(読めるファイルも、書きかけの秘密も残らない)。

use anyhow::{Context, Result};
use std::io::Write;
use std::path::{Path, PathBuf};

/// `path` に `contents` を 0600 で(ディレクトリは 0700)原子的に書く。
pub fn write(path: &Path, contents: &[u8]) -> Result<()> {
    write_with(path, contents, |_| Ok(()))
}

/// [write] の本体。`before_rename(一時ファイル)` は置き換える直前に呼ぶ(テストが途中の状態を見る / 失敗させる)。
pub(crate) fn write_with(path: &Path, contents: &[u8], before_rename: impl FnOnce(&Path) -> Result<()>) -> Result<()> {
    let dir = path.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
    create_private_dir(dir)?;
    let tmp = temp_path(path);
    let result = (|| -> Result<()> {
        let mut f = open_private(&tmp)?;
        f.write_all(contents).with_context(|| format!("{} を書けません", tmp.display()))?;
        f.sync_all()?;
        drop(f);
        before_rename(&tmp)?;
        std::fs::rename(&tmp, path).with_context(|| format!("{} を置き換えられません", path.display()))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

fn temp_path(path: &Path) -> PathBuf {
    let mut rnd = [0u8; 8];
    let _ = getrandom::getrandom(&mut rnd);
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "secret".into());
    let suffix: String = rnd.iter().map(|b| format!("{b:02x}")).collect();
    path.with_file_name(format!(".{name}.{}.{suffix}.tmp", std::process::id()))
}

#[cfg(unix)]
fn create_private_dir(dir: &Path) -> Result<()> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    if !dir.exists() {
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir).with_context(|| format!("{} を作れません", dir.display()))?;
    }
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).with_context(|| format!("{} を 0700 にできません", dir.display()))?;
    Ok(())
}

#[cfg(not(unix))]
fn create_private_dir(dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir).with_context(|| format!("{} を作れません", dir.display()))?;
    Ok(())
}

#[cfg(unix)]
fn open_private(p: &Path) -> Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(p).with_context(|| format!("{} を作れません", p.display()))
}

#[cfg(not(unix))]
fn open_private(p: &Path) -> Result<std::fs::File> {
    std::fs::OpenOptions::new().write(true).create_new(true).open(p).with_context(|| format!("{} を作れません", p.display()))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn scratch(name: &str) -> PathBuf {
        let mut rnd = [0u8; 6];
        getrandom::getrandom(&mut rnd).unwrap();
        let d = std::env::temp_dir().join(format!("rasq-private-{name}-{}", rnd.iter().map(|b| format!("{b:02x}")).collect::<String>()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    fn mode(p: &Path) -> u32 {
        std::fs::metadata(p).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn first_write_is_private_before_any_content_is_visible() {
        // 置き換える前(中身を書いた時点)で、一時ファイルは 0600、ディレクトリは 0700、本物のファイルはまだ無い。
        let dir = scratch("first").join("rasq");
        let file = dir.join("credentials.toml");
        write_with(&file, b"token = \"secret\"", |tmp| {
            assert_eq!(mode(tmp), 0o600);
            assert_eq!(mode(&dir), 0o700);
            assert!(!file.exists());
            Ok(())
        })
        .unwrap();
        assert_eq!(mode(&file), 0o600);
        assert_eq!(std::fs::read(&file).unwrap(), b"token = \"secret\"");
        // 上書きも同じ(置き換えなので、古いファイルの権限を引き継がない)。
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
        write(&file, b"new").unwrap();
        assert_eq!(mode(&file), 0o600);
        assert_eq!(std::fs::read(&file).unwrap(), b"new");
        std::fs::remove_dir_all(dir.parent().unwrap()).unwrap();
    }

    #[test]
    fn interrupted_writes_leave_no_secret_behind() {
        let dir = scratch("interrupted");
        let file = dir.join("dev-key.toml");
        write(&file, b"old").unwrap();
        let err = write_with(&file, b"secret seed", |_| anyhow::bail!("interrupted"));
        assert!(err.is_err());
        // 古い中身のまま、一時ファイルは残らない。
        assert_eq!(std::fs::read(&file).unwrap(), b"old");
        let left: Vec<_> = std::fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()).map(|e| e.file_name()).collect();
        assert_eq!(left.len(), 1, "{left:?}");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
