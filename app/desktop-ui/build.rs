//! Embeds the application icon and version information into the Windows
//! executable. The icon is drawn by the same code used at run time
//! (`src/icon.rs`), so the repository needs no binary image assets.

#[path = "src/icon.rs"]
#[allow(dead_code)]
mod icon;

use std::io::Write;
use std::path::Path;

/// Writes a multi-size .ico (32-bit BMP entries).
fn write_ico(path: &Path, sizes: &[u32]) -> std::io::Result<()> {
    let mut images: Vec<Vec<u8>> = Vec::new();
    for &s in sizes {
        let rgba = icon::rgba(s);
        let mut bmp = Vec::new();
        // BITMAPINFOHEADER (height doubled: XOR + AND masks).
        bmp.extend_from_slice(&40u32.to_le_bytes());
        bmp.extend_from_slice(&(s as i32).to_le_bytes());
        bmp.extend_from_slice(&((s * 2) as i32).to_le_bytes());
        bmp.extend_from_slice(&1u16.to_le_bytes());
        bmp.extend_from_slice(&32u16.to_le_bytes());
        bmp.extend_from_slice(&[0u8; 24]);
        // Pixels bottom-up, BGRA.
        for y in (0..s).rev() {
            for x in 0..s {
                let i = ((y * s + x) * 4) as usize;
                bmp.extend_from_slice(&[rgba[i + 2], rgba[i + 1], rgba[i], rgba[i + 3]]);
            }
        }
        // AND mask (all zero, alpha channel is used), rows padded to 32 bits.
        let row = s.div_ceil(32) * 4;
        bmp.extend(std::iter::repeat_n(0u8, (row * s) as usize));
        images.push(bmp);
    }
    let mut f = std::fs::File::create(path)?;
    f.write_all(&[0, 0, 1, 0])?;
    f.write_all(&(sizes.len() as u16).to_le_bytes())?;
    let mut offset = 6 + 16 * sizes.len() as u32;
    for (s, img) in sizes.iter().zip(&images) {
        let dim = if *s >= 256 { 0 } else { *s as u8 };
        f.write_all(&[dim, dim, 0, 0])?;
        f.write_all(&1u16.to_le_bytes())?;
        f.write_all(&32u16.to_le_bytes())?;
        f.write_all(&(img.len() as u32).to_le_bytes())?;
        f.write_all(&offset.to_le_bytes())?;
        offset += img.len() as u32;
    }
    for img in &images {
        f.write_all(img)?;
    }
    Ok(())
}

fn main() {
    println!("cargo:rerun-if-changed=src/icon.rs");
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let ico = out.join("novamix.ico");
    if let Err(e) = write_ico(&ico, &[16, 24, 32, 48, 64, 128, 256]) {
        println!("cargo:warning=could not write icon: {e}");
        return;
    }
    let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".into());
    let mut parts: Vec<u32> = version.split(['.', '-']).filter_map(|p| p.parse().ok()).collect();
    parts.resize(4, 0);
    let numeric = format!("{},{},{},{}", parts[0], parts[1], parts[2], parts[3]);
    let rc = format!(
        r#"1 ICON "{ico}"
1 VERSIONINFO
FILEVERSION {numeric}
PRODUCTVERSION {numeric}
FILEOS 0x40004
FILETYPE 0x1
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904B0"
    BEGIN
      VALUE "CompanyName", "NovaMix contributors"
      VALUE "FileDescription", "NovaMix - virtual audio mixer"
      VALUE "FileVersion", "{version}"
      VALUE "InternalName", "novamix"
      VALUE "OriginalFilename", "novamix.exe"
      VALUE "ProductName", "NovaMix"
      VALUE "ProductVersion", "{version}"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#,
        ico = ico.display().to_string().replace('\\', "\\\\"),
    );
    let rc_path = out.join("novamix.rc");
    if std::fs::write(&rc_path, rc).is_ok()
        && let Err(e) = embed_resource::compile(&rc_path, embed_resource::NONE).manifest_optional()
    {
        println!("cargo:warning=resource compilation skipped: {e}");
    }
}
