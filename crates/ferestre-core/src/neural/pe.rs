//! A minimal PE reader: the machine, the optional header's magic and the names
//! of the imported DLLs. Enough to tell a game's bitness and graphics API.
//!
//! Seek-based and bounded: it reads the headers, the section table and the
//! import directory, never the whole image, so a 4 GB executable costs a few
//! kilobytes of reads, and a damaged or hostile file is an error, not a panic
//! or an unbounded loop.

use anyhow::{anyhow, bail, Context, Result};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// What an executable tells about itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeInfo {
    /// 32 or 64.
    pub bits: u8,
    /// The imported DLLs' names, lowercase, in the order the image lists them.
    pub imports: Vec<String>,
}

const MAX_DESCRIPTORS: usize = 4096;
const MAX_NAME: usize = 256;
/// The PE format allows at most 96 sections.
const MAX_SECTIONS: u16 = 96;

const MACHINE_I386: u16 = 0x14c;
const MACHINE_AMD64: u16 = 0x8664;
const MAGIC_PE32: u16 = 0x10b;
const MAGIC_PE32_PLUS: u16 = 0x20b;

struct Section {
    virtual_address: u32,
    virtual_size: u32,
    raw_size: u32,
    raw_pointer: u32,
}

fn u16_at(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

fn read_at(file: &mut File, offset: u64, buf: &mut [u8]) -> Result<()> {
    file.seek(SeekFrom::Start(offset))?;
    file.read_exact(buf)
        .map_err(|e| anyhow!("truncated at byte {offset}: {e}"))
}

/// The file offset of `rva`, if a section's file data holds it.
fn offset_of(sections: &[Section], rva: u32) -> Option<u64> {
    sections.iter().find_map(|s| {
        let span = s.virtual_size.max(s.raw_size);
        let inside = rva >= s.virtual_address
            && u64::from(rva) < u64::from(s.virtual_address) + u64::from(span);
        let delta = rva.checked_sub(s.virtual_address)?;
        (inside && delta < s.raw_size).then(|| u64::from(s.raw_pointer) + u64::from(delta))
    })
}

pub fn read(path: &Path) -> Result<PeInfo> {
    let mut file = File::open(path).with_context(|| format!("{}", path.display()))?;
    read_file(&mut file).with_context(|| format!("{} is not a readable PE image", path.display()))
}

fn read_file(file: &mut File) -> Result<PeInfo> {
    let mut dos = [0u8; 64];
    read_at(file, 0, &mut dos)?;
    if &dos[..2] != b"MZ" {
        bail!("no MZ header");
    }
    let pe = u64::from(u32_at(&dos, 0x3c));
    let mut coff = [0u8; 24];
    read_at(file, pe, &mut coff)?;
    if &coff[..4] != b"PE\0\0" {
        bail!("no PE signature");
    }
    let machine = u16_at(&coff, 4);
    let sections = u16_at(&coff, 6);
    let optional_size = u16_at(&coff, 20);
    let (bits, wanted_magic, directories_at) = match machine {
        MACHINE_I386 => (32, MAGIC_PE32, 96usize),
        MACHINE_AMD64 => (64, MAGIC_PE32_PLUS, 112usize),
        other => bail!("machine {other:#x} is neither x86 nor x86-64"),
    };
    if sections > MAX_SECTIONS {
        bail!("{sections} sections");
    }
    let optional_at = pe + 24;
    let mut optional = vec![0u8; usize::from(optional_size)];
    read_at(file, optional_at, &mut optional)?;
    if optional.len() < 2 {
        bail!("no optional header");
    }
    let magic = u16_at(&optional, 0);
    if magic != wanted_magic {
        bail!("optional header magic {magic:#x} does not match machine {machine:#x}");
    }
    // The import directory is the second data directory.
    if optional.len() < directories_at {
        bail!("optional header too short");
    }
    let count = u32_at(&optional, directories_at - 4);
    let import_at = directories_at + 8;
    if count < 2 || optional.len() < import_at + 8 {
        return Ok(PeInfo {
            bits,
            imports: Vec::new(),
        });
    }
    let import_rva = u32_at(&optional, import_at);
    if import_rva == 0 {
        return Ok(PeInfo {
            bits,
            imports: Vec::new(),
        });
    }

    let mut table = vec![0u8; usize::from(sections) * 40];
    read_at(file, optional_at + u64::from(optional_size), &mut table)?;
    let sections: Vec<Section> = (0..table.len() / 40)
        .map(|i| &table[i * 40..])
        .map(|s| Section {
            virtual_size: u32_at(s, 8),
            virtual_address: u32_at(s, 12),
            raw_size: u32_at(s, 16),
            raw_pointer: u32_at(s, 20),
        })
        .collect();

    let mut at = offset_of(&sections, import_rva)
        .ok_or_else(|| anyhow!("the import directory is in no section"))?;
    let mut imports = Vec::new();
    for _ in 0..MAX_DESCRIPTORS {
        let mut descriptor = [0u8; 20];
        read_at(file, at, &mut descriptor)?;
        if descriptor.iter().all(|&b| b == 0) {
            return Ok(PeInfo { bits, imports });
        }
        let name_rva = u32_at(&descriptor, 12);
        let name_at = offset_of(&sections, name_rva)
            .ok_or_else(|| anyhow!("an import name is in no section"))?;
        imports.push(read_name(file, name_at)?);
        at += 20;
    }
    bail!("more than {MAX_DESCRIPTORS} imports")
}

fn read_name(file: &mut File, at: u64) -> Result<String> {
    file.seek(SeekFrom::Start(at))?;
    let mut buf = Vec::with_capacity(64);
    file.take(MAX_NAME as u64 + 1).read_to_end(&mut buf)?;
    let end = buf
        .iter()
        .position(|&b| b == 0)
        .ok_or_else(|| anyhow!("an import name is unterminated or longer than {MAX_NAME} bytes"))?;
    let name =
        std::str::from_utf8(&buf[..end]).map_err(|_| anyhow!("an import name is not text"))?;
    Ok(name.to_ascii_lowercase())
}

/// A tiny PE image, built in tests so no binary fixture is in the repository.
#[cfg(test)]
pub(crate) fn tiny_pe(machine: u16, magic: u16, imports: &[&str]) -> Vec<u8> {
    const SECTION_VA: u32 = 0x1000;
    const SECTION_AT: usize = 0x200;
    let optional_size: u16 = if magic == MAGIC_PE32_PLUS { 240 } else { 224 };
    let directories_at = if magic == MAGIC_PE32_PLUS { 112 } else { 96 };

    // The section: descriptors, a zero terminator, then the names.
    let mut data = vec![0u8; (imports.len() + 1) * 20];
    for (i, name) in imports.iter().enumerate() {
        let name_rva = SECTION_VA + data.len() as u32;
        data[i * 20 + 12..i * 20 + 16].copy_from_slice(&name_rva.to_le_bytes());
        data.extend_from_slice(name.as_bytes());
        data.push(0);
    }

    let mut image = vec![0u8; SECTION_AT];
    image[..2].copy_from_slice(b"MZ");
    image[0x3c..0x40].copy_from_slice(&0x40u32.to_le_bytes());
    let pe = 0x40;
    image[pe..pe + 4].copy_from_slice(b"PE\0\0");
    image[pe + 4..pe + 6].copy_from_slice(&machine.to_le_bytes());
    image[pe + 6..pe + 8].copy_from_slice(&1u16.to_le_bytes());
    image[pe + 20..pe + 22].copy_from_slice(&optional_size.to_le_bytes());
    let opt = pe + 24;
    image[opt..opt + 2].copy_from_slice(&magic.to_le_bytes());
    image[opt + directories_at - 4..opt + directories_at].copy_from_slice(&16u32.to_le_bytes());
    let import = opt + directories_at + 8;
    image[import..import + 4].copy_from_slice(&SECTION_VA.to_le_bytes());
    image[import + 4..import + 8].copy_from_slice(&(data.len() as u32).to_le_bytes());
    let section = opt + usize::from(optional_size);
    image[section..section + 6].copy_from_slice(b".idata");
    let size = data.len() as u32;
    image[section + 8..section + 12].copy_from_slice(&size.to_le_bytes());
    image[section + 12..section + 16].copy_from_slice(&SECTION_VA.to_le_bytes());
    image[section + 16..section + 20].copy_from_slice(&size.to_le_bytes());
    image[section + 20..section + 24].copy_from_slice(&(SECTION_AT as u32).to_le_bytes());
    image.extend_from_slice(&data);
    image
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(bytes: &[u8]) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Game.exe");
        std::fs::write(&path, bytes).unwrap();
        (dir, path)
    }

    #[test]
    fn a_64_bit_image_lists_its_imports() {
        let (_dir, path) = file(&tiny_pe(0x8664, 0x20b, &["KERNEL32.dll", "d3d12.dll"]));
        let info = read(&path).unwrap();
        assert_eq!(info.bits, 64);
        assert_eq!(info.imports, vec!["kernel32.dll", "d3d12.dll"]);
    }

    #[test]
    fn a_32_bit_image_is_read_too() {
        let (_dir, path) = file(&tiny_pe(0x14c, 0x10b, &["d3d9.dll"]));
        let info = read(&path).unwrap();
        assert_eq!(info.bits, 32);
        assert_eq!(info.imports, vec!["d3d9.dll"]);
        // A machine that is neither, or a magic that does not match it.
        let (_dir, path) = file(&tiny_pe(0xaa64, 0x20b, &[]));
        assert!(read(&path).is_err());
        let (_dir, path) = file(&tiny_pe(0x8664, 0x10b, &[]));
        assert!(read(&path).is_err());
    }

    #[test]
    fn garbage_and_truncated_files_are_errors_not_panics() {
        let (_dir, path) = file(b"MZ");
        assert!(read(&path).is_err());

        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let random: Vec<u8> = (0..64)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                seed as u8
            })
            .collect();
        let (_dir, path) = file(&random);
        assert!(read(&path).is_err());

        // An import RVA past the end of the file.
        let mut image = tiny_pe(0x8664, 0x20b, &["d3d12.dll"]);
        let import = 0x40 + 24 + 112 + 8;
        image[import..import + 4].copy_from_slice(&0x7fff_0000u32.to_le_bytes());
        let (_dir, path) = file(&image);
        assert!(read(&path).is_err());

        // The image cut short inside its import table.
        let image = tiny_pe(0x8664, 0x20b, &["d3d12.dll"]);
        let (_dir, path) = file(&image[..0x200 + 10]);
        assert!(read(&path).is_err());

        // Every prefix of a valid image: an error or a result, never a panic.
        let image = tiny_pe(0x14c, 0x10b, &["kernel32.dll", "d3d9.dll"]);
        for end in 0..image.len() {
            let (_dir, path) = file(&image[..end]);
            let _ = read(&path);
        }
    }
}
