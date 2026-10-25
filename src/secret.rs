//! The secret downloads.

use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, PartialEq)]
pub struct SecretFile {
    pub name: String,
    pub size: u64,
}

pub fn dir() -> PathBuf {
    folder(&crate::store::data_dir())
}

fn folder(data: &Path) -> PathBuf {
    let d = data.join("secret");
    if !d.exists() {
        let _ = fs::create_dir_all(&d);
        hide(&d);
    }
    d
}

fn hide(p: &Path) {
    use windows::{
        Win32::Storage::FileSystem::{FILE_ATTRIBUTE_HIDDEN, SetFileAttributesW},
        core::HSTRING,
    };
    unsafe {
        let _ = SetFileAttributesW(&HSTRING::from(p.as_os_str()), FILE_ATTRIBUTE_HIDDEN);
    }
}

pub fn move_in(file: &Path) -> std::io::Result<PathBuf> {
    move_into(file, &dir())
}

pub fn move_out(name: &str) -> std::io::Result<PathBuf> {
    let from = dir().join(plain(name)?);
    move_into(&from, &crate::store::download_dir())
}

pub fn list() -> Vec<SecretFile> {
    list_in(&dir())
}

fn list_in(d: &Path) -> Vec<SecretFile> {
    let mut v: Vec<SecretFile> = fs::read_dir(d)
        .map(|rd| {
            rd.flatten()
                .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
                .map(|e| SecretFile { name: e.file_name().to_string_lossy().into_owned(), size: e.metadata().map_or(0, |m| m.len()) })
                .collect()
        })
        .unwrap_or_default();
    v.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    v
}

pub fn path_of(name: &str) -> std::io::Result<PathBuf> {
    Ok(dir().join(plain(name)?))
}

fn plain(name: &str) -> std::io::Result<&str> {
    let ok = !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\', ':']);
    if ok { Ok(name) } else { Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "not a file name")) }
}

fn move_into(file: &Path, to: &Path) -> std::io::Result<PathBuf> {
    let name = file.file_name().ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "no file name"))?;
    let dest = free_name(to, &name.to_string_lossy());
    if fs::rename(file, &dest).is_err() {
        fs::copy(file, &dest)?;
        fs::remove_file(file)?;
    }
    Ok(dest)
}

fn free_name(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s.to_string(), format!(".{e}")),
        _ => (name.to_string(), String::new()),
    };
    (2..)
        .map(|n| dir.join(format!("{stem} ({n}){ext}")))
        .find(|p| !p.exists())
        .unwrap_or(first)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("lumen-secret-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        let _ = fs::create_dir_all(&d);
        d
    }

    #[test]
    fn a_file_goes_in_and_comes_back_under_a_free_name() {
        let root = scratch("move");
        let secret = folder(&root);
        let downloads = root.join("dl");
        let _ = fs::create_dir_all(&downloads);
        let f = downloads.join("report.pdf");
        fs::write(&f, b"pdf").unwrap();
        let inside = move_into(&f, &secret).unwrap();
        assert!(!f.exists() && inside.exists());
        assert_eq!(list_in(&secret), vec![SecretFile { name: "report.pdf".into(), size: 3 }]);
        fs::write(&f, b"new").unwrap();
        let back = move_into(&inside, &downloads).unwrap();
        assert_eq!(back.file_name().unwrap(), "report (2).pdf");
        assert_eq!(fs::read(&f).unwrap(), b"new");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_name_from_the_page_never_climbs_out() {
        assert!(plain("a.zip").is_ok());
        for bad in ["", "..", "../x", "a\\b", "C:x", "."] {
            assert!(plain(bad).is_err(), "{bad} got through");
        }
    }
}
