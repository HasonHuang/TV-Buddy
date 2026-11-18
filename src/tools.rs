//! 外部工具路径：面板安装的版本优先，系统 PATH 兜底。

use std::path::{Path, PathBuf};

pub mod install;

/// 数据目录：数据库文件所在目录。
pub fn data_dir(db_path: &Path) -> PathBuf {
    db_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// 面板安装的工具目录。
pub fn dir(data_dir: &Path) -> PathBuf {
    data_dir.join("tools")
}

/// Unix 可执行判定：普通文件且任一执行位为 1。
pub fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.is_file()
        && std::fs::metadata(path)
            .map(|meta| meta.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
}

/// 面板安装的二进制；不存在或不可执行时返回 `None`。
pub fn managed_path(data_dir: &Path, name: &str) -> Option<PathBuf> {
    let path = dir(data_dir).join(name);
    is_executable(&path).then_some(path)
}

pub fn ffmpeg(data_dir: &Path) -> PathBuf {
    managed_path(data_dir, "ffmpeg").unwrap_or_else(|| PathBuf::from("ffmpeg"))
}

pub fn ffprobe(data_dir: &Path) -> PathBuf {
    managed_path(data_dir, "ffprobe").unwrap_or_else(|| PathBuf::from("ffprobe"))
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::{fs, path::PathBuf};

    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tv-buddy-tools-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).expect("建临时目录");
        dir
    }

    fn executable(path: &std::path::Path) {
        let mut permissions = fs::metadata(path).expect("读属性").permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions).expect("设权限");
    }

    #[test]
    fn managed_binary_wins_over_path() {
        let dir = temp_dir();
        fs::create_dir_all(super::dir(&dir)).expect("建 tools");
        let managed = super::dir(&dir).join("ffmpeg");
        fs::write(&managed, b"#!/bin/sh\n").expect("写文件");
        executable(&managed);
        assert_eq!(super::ffmpeg(&dir), managed);
        assert_eq!(super::ffprobe(&dir), PathBuf::from("ffprobe"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn missing_or_non_executable_falls_back_to_path() {
        let dir = temp_dir();
        assert_eq!(super::ffmpeg(&dir), PathBuf::from("ffmpeg"));

        fs::create_dir_all(super::dir(&dir)).expect("建 tools");
        let managed = super::dir(&dir).join("ffmpeg");
        fs::write(&managed, b"#!/bin/sh\n").expect("写文件");
        fs::set_permissions(&managed, fs::Permissions::from_mode(0o644)).expect("设权限");
        assert_eq!(super::ffmpeg(&dir), PathBuf::from("ffmpeg"));
        let _ = fs::remove_dir_all(dir);
    }
}
