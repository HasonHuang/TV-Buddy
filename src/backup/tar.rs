//! 最小 ustar 读写。
//!
//! 只支持普通文件；文件名 < 100 字节，大小 < 8 GiB。
//! 这样备份不引入第三方 tar 库，交叉编译无需新依赖（F-013 §8）。

use anyhow::{bail, Result};

/// 一个文件条目。
pub struct Entry {
    pub name: String,
    pub data: Vec<u8>,
}

/// 打包成 tar 字节流（末尾两个全零块）。
pub fn build(entries: &[Entry]) -> Vec<u8> {
    let mut out = Vec::new();
    for entry in entries {
        out.extend_from_slice(&header(&entry.name, entry.data.len() as u64));
        out.extend_from_slice(&entry.data);
        let pad = (512 - entry.data.len() % 512) % 512;
        out.extend(std::iter::repeat(0u8).take(pad));
    }
    out.extend_from_slice(&[0u8; 1024]);
    out
}

/// 解包；遇到全零块或数据不足时停止。
pub fn parse(data: &[u8]) -> Result<Vec<Entry>> {
    let mut entries = Vec::new();
    let mut offset = 0usize;
    while offset + 512 <= data.len() {
        let block = &data[offset..offset + 512];
        if block.iter().all(|byte| *byte == 0) {
            break;
        }
        let name = cstr(&block[0..100]);
        let size = octal(&block[124..136]) as usize;
        let typeflag = block[156];
        offset += 512;
        if offset + size > data.len() {
            bail!("tar 条目越界：{name}");
        }
        let payload = &data[offset..offset + size];
        offset += size.div_ceil(512) * 512;
        if typeflag == b'0' || typeflag == 0 {
            entries.push(Entry {
                name,
                data: payload.to_vec(),
            });
        }
    }
    Ok(entries)
}

fn header(name: &str, size: u64) -> [u8; 512] {
    let mut block = [0u8; 512];
    let bytes = name.as_bytes();
    let len = bytes.len().min(100);
    block[..len].copy_from_slice(&bytes[..len]);
    octal_set(&mut block[100..108], 0o644);
    octal_set(&mut block[108..116], 0);
    octal_set(&mut block[116..124], 0);
    octal_set(&mut block[124..136], size);
    octal_set(&mut block[136..148], unix_now());
    block[148..156].fill(b' ');
    block[156] = b'0';
    block[257..263].copy_from_slice(b"ustar\0");
    block[263..265].copy_from_slice(b"00");
    let sum: u32 = block.iter().map(|byte| u32::from(*byte)).sum();
    block[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());
    block
}

fn octal_set(field: &mut [u8], value: u64) {
    let width = field.len() - 1;
    let text = format!("{value:0width$o}");
    let bytes = text.as_bytes();
    let len = bytes.len().min(width);
    field[..len].copy_from_slice(&bytes[..len]);
    field[field.len() - 1] = 0;
}

fn octal(field: &[u8]) -> u64 {
    let text: String = field
        .iter()
        .take_while(|byte| **byte != 0 && **byte != b' ')
        .map(|byte| *byte as char)
        .collect();
    u64::from_str_radix(text.trim(), 8).unwrap_or(0)
}

fn cstr(field: &[u8]) -> String {
    let end = field
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(field.len());
    String::from_utf8_lossy(&field[..end]).to_string()
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let entries = vec![
            Entry {
                name: "tv-buddy-backup/config.json".into(),
                data: b"{}".to_vec(),
            },
            Entry {
                name: "tv-buddy-backup/logos/a.png".into(),
                data: vec![0x89, 0x50, 0x4e, 0x47, 1, 2, 3],
            },
            Entry {
                name: "empty.txt".into(),
                data: Vec::new(),
            },
        ];
        let data = build(&entries);
        assert_eq!(data.len() % 512, 0);
        let parsed = parse(&data).expect("解析");
        assert_eq!(parsed.len(), 3);
        assert_eq!(parsed[0].name, "tv-buddy-backup/config.json");
        assert_eq!(parsed[1].data, vec![0x89, 0x50, 0x4e, 0x47, 1, 2, 3]);
        assert!(parsed[2].data.is_empty());
    }

    #[test]
    fn rejects_truncated_payload() {
        let mut data = build(&[Entry {
            name: "a".into(),
            data: vec![1; 600],
        }]);
        data.truncate(700);
        assert!(parse(&data).is_err());
    }
}
