//! 本机文件库：内容寻址存储（CAS）。
//!
//! 文件字节只存一份，路径由 SHA-256 决定；数据库只存元数据与关系。
//! 这里的所有函数都不修改原始文件。

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

pub const CHUNK_SIZE: usize = 1024 * 1024;

/// 计算文件的 SHA-256（流式，避免把大 PDF 整个读进内存）。
pub fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; CHUNK_SIZE];

    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }

    Ok(hex::encode(hasher.finalize()))
}

/// 内容寻址的目标相对路径：blobs/<前两位>/<sha256>.<ext>
pub fn blob_rel_path(sha256: &str, ext: &str) -> PathBuf {
    let prefix = sha256.get(0..2).unwrap_or("00");
    let name = if ext.is_empty() {
        sha256.to_string()
    } else {
        format!("{sha256}.{ext}")
    };
    PathBuf::from("library").join("blobs").join(prefix).join(name)
}

/// 可读的文件大小。
pub fn human_size(bytes: i64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0usize;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_temp(name: &str, content: &[u8]) -> PathBuf {
        let path = std::env::temp_dir().join(format!("meetingdesk-test-{name}"));
        let mut file = File::create(&path).expect("创建临时文件");
        file.write_all(content).expect("写入临时文件");
        path
    }

    #[test]
    fn sha256_与已知值一致() {
        let path = write_temp("hash.txt", b"abc");
        let hash = sha256_file(&path).expect("计算哈希");
        assert_eq!(
            hash,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn 相同内容得到相同哈希() {
        let a = write_temp("same-a.txt", b"budget-2026");
        let b = write_temp("same-b.txt", b"budget-2026");
        assert_eq!(sha256_file(&a).unwrap(), sha256_file(&b).unwrap());
        let _ = std::fs::remove_file(a);
        let _ = std::fs::remove_file(b);
    }

    #[test]
    fn 分片路径按哈希前缀组织() {
        let path = blob_rel_path("3f8ac21deadbeef", "pdf");
        assert_eq!(path.to_string_lossy(), "library/blobs/3f/3f8ac21deadbeef.pdf");
    }
}
