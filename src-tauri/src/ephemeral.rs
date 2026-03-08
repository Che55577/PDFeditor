//! 暫存檔生命週期管理。
//!
//! 這是本產品的核心承諾所在：處理過程中產生的檔案，在使用者存檔（或放棄）後
//! 立刻消失，而且程式當掉也不會留下殘骸。
//!
//! 誠實聲明：在 SSD / 手機快閃記憶體上，因為 wear-leveling 與 TRIM，
//! 「覆寫刪除」**無法保證**原始資料真的被抹除。這裡的覆寫是盡力而為。
//! 真正的保障是：暫存存在時間極短、範圍受控、且全程不連網。

use std::fs::{self, File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// 一次寫入的歸零區塊大小。
const WIPE_CHUNK: usize = 1 << 20;

/// 暫存目錄的根。所有落地的中間檔案都只會出現在這底下，方便一次掃乾淨。
pub fn root(app_data: &Path) -> PathBuf {
    app_data.join("eph")
}

/// 啟動時呼叫：清掉上次當掉留下的殘骸。
///
/// 刻意不回報錯誤——清不掉（例如檔案被佔用）不該擋住 App 啟動，
/// 但每一次啟動都會再試一次。
pub fn sweep(root: &Path) {
    let Ok(entries) = fs::read_dir(root) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            wipe_dir_all(&path);
        } else {
            wipe(&path);
        }
    }
}

/// 遞迴將資料夾內所有檔案覆寫一輪後再移除整個目錄樹。
pub fn wipe_dir_all(dir: &Path) {
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                wipe_dir_all(&path);
            } else {
                wipe(&path);
            }
        }
    }
    let _ = fs::remove_dir_all(dir);
}

/// 覆寫一輪後刪除。失敗時仍會嘗試直接刪除。
pub fn wipe(path: &Path) {
    if let Ok(meta) = fs::metadata(path) {
        let len = meta.len();
        if let Ok(mut f) = OpenOptions::new().write(true).open(path) {
            let zeros = vec![0u8; WIPE_CHUNK.min(len.max(1) as usize)];
            let mut left = len;
            let _ = f.seek(SeekFrom::Start(0));
            while left > 0 {
                let n = (left as usize).min(zeros.len());
                if f.write_all(&zeros[..n]).is_err() {
                    break;
                }
                left -= n as u64;
            }
            let _ = f.sync_all();
        }
    }
    let _ = fs::remove_file(path);
}

/// 一個暫存檔。Drop 時自動抹除——包含 panic 展開的情況。
///
/// 要保留檔案（例如已經搬到使用者選定的位置）請呼叫 [`Ephemeral::release`]。
pub struct Ephemeral {
    path: PathBuf,
    armed: bool,
}

impl Ephemeral {
    /// 在暫存根目錄下建立一個亂數命名、無副檔名的檔案位置。
    ///
    /// 不帶副檔名是刻意的：避免檔案總管與索引服務去讀它的內容產生縮圖或摘要。
    pub fn new(root: &Path) -> std::io::Result<Self> {
        fs::create_dir_all(root)?;
        let name = format!("{:032x}", rand_u128());
        let path = root.join(name);
        File::create(&path)?;
        Ok(Self { path, armed: true })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 交出檔案並解除自動抹除。呼叫後這個暫存檔不再受管理。
    pub fn release(mut self) -> PathBuf {
        self.armed = false;
        self.path.clone()
    }

    /// 立刻抹除，不等 Drop。
    pub fn destroy(mut self) {
        self.armed = false;
        wipe(&self.path);
    }
}

impl Drop for Ephemeral {
    fn drop(&mut self) {
        if self.armed {
            wipe(&self.path);
        }
    }
}

impl std::fmt::Debug for Ephemeral {
    /// 刻意不印出路徑。暫存檔名不該只因為某處寫了 `dbg!` 或錯誤訊息就流進日誌。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Ephemeral")
            .field("armed", &self.armed)
            .finish_non_exhaustive()
    }
}

/// 產生亂數檔名用的 128 bit。不需要密碼學強度，只要不撞名、不可預測即可，
/// 為此拉一個 rand crate 進來不划算。
fn rand_u128() -> u128 {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    let hi = RandomState::new().build_hasher().finish() as u128;
    let lo = RandomState::new().build_hasher().finish() as u128;
    (hi << 64) | lo
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_root() -> PathBuf {
        let p = std::env::temp_dir().join(format!("localpdf-test-{:032x}", rand_u128()));
        fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn drop_removes_file() {
        let root = tmp_root();
        let path = {
            let e = Ephemeral::new(&root).unwrap();
            fs::write(e.path(), b"sensitive").unwrap();
            e.path().to_path_buf()
        };
        assert!(!path.exists(), "Ephemeral 離開作用域後檔案應該消失");
    }

    #[test]
    fn release_keeps_file() {
        let root = tmp_root();
        let e = Ephemeral::new(&root).unwrap();
        fs::write(e.path(), b"keep me").unwrap();
        let path = e.release();
        assert!(path.exists(), "release 之後檔案應該保留");
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn survives_panic() {
        let root = tmp_root();
        let leaked = std::sync::Arc::new(std::sync::Mutex::new(PathBuf::new()));
        let l2 = leaked.clone();
        let _ = std::panic::catch_unwind(move || {
            let e = Ephemeral::new(&root).unwrap();
            fs::write(e.path(), b"boom").unwrap();
            *l2.lock().unwrap() = e.path().to_path_buf();
            panic!("模擬處理中途當掉");
        });
        let path = leaked.lock().unwrap().clone();
        assert!(!path.exists(), "panic 展開時 Drop 仍須抹除暫存檔");
    }

    #[test]
    fn sweep_clears_root() {
        let root = tmp_root();
        for _ in 0..3 {
            let e = Ephemeral::new(&root).unwrap();
            fs::write(e.path(), b"junk").unwrap();
            let _ = e.release();
        }
        assert_eq!(fs::read_dir(&root).unwrap().count(), 3);
        sweep(&root);
        assert_eq!(fs::read_dir(&root).unwrap().count(), 0, "sweep 應清空殘骸");
    }
}
