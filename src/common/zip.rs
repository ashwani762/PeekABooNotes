// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! A small zip writer, for sharing notes as one file. Entries are stored, not compressed:
//! notes are a few kilobytes and pictures are compressed already, so a compressor would
//! add size to the app for next to nothing in the zip.

use std::io::{self, Write};
use std::path::{Path, PathBuf};

use super::util::LocalTime;

const TABLE: [u32; 256] = {
    let mut t = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            k += 1;
        }
        t[i] = c;
        i += 1;
    }
    t
};

fn crc32(data: &[u8]) -> u32 {
    !data.iter().fold(!0u32, |c, &b| TABLE[((c ^ b as u32) & 0xFF) as usize] ^ (c >> 8))
}

/// MS-DOS time and date (two-second steps): local wall-clock time, as zip tools show it.
fn dos_time(t: &LocalTime) -> (u16, u16) {
    let year = t.year.clamp(1980, 2107) as u32;
    ((t.hour << 11 | t.minute << 5 | (t.second / 2)) as u16, ((year - 1980) << 9 | t.month << 5 | t.day) as u16)
}

/// A file's modified time, local.
fn modified(path: &Path) -> LocalTime {
    let t = std::fs::metadata(path).and_then(|m| m.modified()).ok();
    let secs = t.and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
    crate::os::local_time(secs as i64)
}

fn collect(root: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) -> io::Result<()> {
    for e in std::fs::read_dir(dir)? {
        let p = e?.path();
        if p.is_dir() {
            collect(root, &p, out)?;
        } else if let Ok(rel) = p.strip_prefix(root) {
            let name = rel.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/");
            out.push((name, p));
        }
    }
    Ok(())
}

/// Zip every file under `root`, named by their paths inside it, into `out`. How many went in.
pub fn zip_dir(root: &Path, out: &Path) -> io::Result<usize> {
    let mut files = Vec::new();
    collect(root, root, &mut files)?;
    files.sort();
    let mut w = io::BufWriter::new(std::fs::File::create(out)?);
    let (mut central, mut offset) = (Vec::new(), 0u64);
    for (name, path) in &files {
        let data = std::fs::read(path)?;
        if data.len() >= u32::MAX as usize || offset + data.len() as u64 >= u32::MAX as u64 {
            return Err(io::Error::other("too large to zip"));
        }
        let (n, crc, size) = (name.as_bytes(), crc32(&data), data.len() as u32);
        let (time, date) = dos_time(&modified(path));
        // Version 2.0, UTF-8 names (flag bit 11), stored.
        let common = |v: &mut Vec<u8>| {
            for x in [20u16, 0x0800, 0, time, date] {
                v.extend(x.to_le_bytes());
            }
            for x in [crc, size, size] {
                v.extend(x.to_le_bytes());
            }
            v.extend((n.len() as u16).to_le_bytes());
            v.extend(0u16.to_le_bytes());
        };
        let mut local = 0x0403_4b50u32.to_le_bytes().to_vec();
        common(&mut local);
        local.extend(n);
        w.write_all(&local)?;
        w.write_all(&data)?;
        central.extend(0x0201_4b50u32.to_le_bytes());
        central.extend(20u16.to_le_bytes());
        common(&mut central);
        central.extend([0u8; 10]); // comment length, disk, internal and external attributes
        central.extend((offset as u32).to_le_bytes());
        central.extend(n);
        offset += (local.len() + data.len()) as u64;
    }
    w.write_all(&central)?;
    let count = files.len() as u16;
    let mut end = 0x0605_4b50u32.to_le_bytes().to_vec();
    for x in [0u16, 0, count, count] {
        end.extend(x.to_le_bytes());
    }
    end.extend((central.len() as u32).to_le_bytes());
    end.extend((offset as u32).to_le_bytes());
    end.extend(0u16.to_le_bytes());
    w.write_all(&end)?;
    w.flush()?;
    Ok(files.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_a_zip() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        let t = LocalTime { year: 2023, month: 11, day: 14, hour: 22, minute: 13, second: 21, millis: 0 };
        assert_eq!(dos_time(&t), (22 << 11 | 13 << 5 | 10, 43 << 9 | 11 << 5 | 14));
        let dir = std::env::temp_dir().join(format!("peekaboo-zip-test-{}", std::process::id()));
        let src = dir.join("src");
        std::fs::create_dir_all(src.join("attachments")).unwrap();
        std::fs::write(src.join("Café.md"), "# Café\n").unwrap();
        std::fs::write(src.join("attachments").join("a.txt"), "hello").unwrap();
        let out = dir.join("out.zip");
        assert_eq!(zip_dir(&src, &out).unwrap(), 2);
        let z = std::fs::read(&out).unwrap();
        let u16_at = |i: usize| u16::from_le_bytes([z[i], z[i + 1]]);
        let u32_at = |i: usize| u32::from_le_bytes([z[i], z[i + 1], z[i + 2], z[i + 3]]);
        // The first entry, in name order, and the directory at the end.
        assert_eq!(u32_at(0), 0x0403_4b50);
        assert_eq!(&z[30..30 + u16_at(26) as usize], "Café.md".as_bytes());
        assert_eq!(u32_at(14), crc32("# Café\n".as_bytes()));
        let end = z.len() - 22;
        assert_eq!(u32_at(end), 0x0605_4b50);
        assert_eq!(u16_at(end + 10), 2);
        let cd = u32_at(end + 16) as usize;
        assert_eq!(u32_at(cd), 0x0201_4b50);
        let second = cd + 46 + u16_at(cd + 28) as usize;
        assert_eq!(&z[second + 46..second + 46 + u16_at(second + 28) as usize], b"attachments/a.txt");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
