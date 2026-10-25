use std::{fs, path::Path, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=lumen.rc");
    println!("cargo:rerun-if-changed=assets/lumen.ico");
    embed_resource::compile("lumen.rc", embed_resource::NONE)
        .manifest_optional()
        .unwrap();
    angle_dlls();
    stamp();
    version();
}

fn version() {
    println!("cargo:rerun-if-changed=Cargo.toml");
    let toml = fs::read_to_string("Cargo.toml").unwrap_or_default();
    let base = std::env::var("CARGO_PKG_VERSION").unwrap_or_default();
    let release = toml
        .split("[package.metadata.lumen]")
        .nth(1)
        .and_then(|t| t.lines().find_map(|l| l.trim().strip_prefix("release").map(|r| r.trim_start_matches([' ', '=']).trim().trim_matches('"').to_string())))
        .filter(|r| r.split('.').all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit())));
    let v = match release {
        Some(r) => format!("{base}.{r}"),
        None => base,
    };
    println!("cargo:rustc-env=LUMEN_VERSION={v}");
}

fn stamp() {
    println!("cargo:rerun-if-changed=.git/logs/HEAD");
    println!("cargo:rerun-if-changed=src");
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
    };
    let commit = git(&["log", "-1", "--format=%h %cd", "--date=format:%Y-%m-%d"]).unwrap_or_else(|| "unknown".into());
    let dirty = git(&["status", "--porcelain", "--untracked-files=no"]).is_some_and(|s| !s.is_empty());
    println!("cargo:rustc-env=LUMEN_BUILD={commit}{}", if dirty { " +dev" } else { "" });
}

fn angle_dlls() {
    if std::env::var_os("CARGO_FEATURE_SERVO").is_none() {
        return;
    }
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap_or_default());
    let Some(profile) = out.ancestors().nth(3) else { return };
    let mut done = 0;
    if let Ok(entries) = fs::read_dir(profile.join("build")) {
        for entry in entries.flatten() {
            if !entry.file_name().to_string_lossy().starts_with("mozangle-") {
                continue;
            }
            for dll in ["libEGL.dll", "libGLESv2.dll"] {
                let from = entry.path().join("out").join(dll);
                if !from.is_file() {
                    continue;
                }
                println!("cargo:rerun-if-changed={}", from.display());
                if put(&from, &profile.join(dll)) {
                    done += 1;
                }
            }
        }
    }
    if done < 2 {
        println!(
            "cargo:warning=the ANGLE DLLs are not built yet, so they are not next to the exe and a Servo \
             tab will not open. Build once more (or copy them by hand; docs/servo-notes.md says how)."
        );
    }
}

fn put(from: &Path, to: &Path) -> bool {
    let Ok(want) = fs::read(from) else { return false };
    if fs::read(to).is_ok_and(|old| old == want) {
        return true;
    }
    fs::copy(from, to).is_ok()
}
