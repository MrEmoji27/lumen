// Bake the icon into the .exe. Explorer, the taskbar and Alt+Tab all read it from here.
use std::{fs, path::Path, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=lumen.rc");
    println!("cargo:rerun-if-changed=assets/lumen.ico");
    embed_resource::compile("lumen.rc", embed_resource::NONE)
        .manifest_optional()
        .unwrap();
    angle_dlls();
}

/// Servo draws through ANGLE, which lives in two DLLs the Windows loader has to find
/// next to the exe. Only a build with the `servo` feature wants them. They come out of
/// mozangle's build directory, which on a first, clean build may not exist yet when
/// this script runs (build scripts are scheduled in parallel): then this says so and
/// the next build copies them. See docs/servo-notes.md for the manual line.
fn angle_dlls() {
    if std::env::var_os("CARGO_FEATURE_SERVO").is_none() {
        return;
    }
    // OUT_DIR is <target>/<profile>/build/lumen-<hash>/out, and the exe lands three
    // levels up from it: that is where the loader looks first.
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
                // Re-run when ANGLE changes, so a fresh build is copied in.
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

/// Copy `from` over `to` unless the bytes are already the same.
fn put(from: &Path, to: &Path) -> bool {
    let Ok(want) = fs::read(from) else { return false };
    if fs::read(to).is_ok_and(|old| old == want) {
        return true;
    }
    fs::copy(from, to).is_ok()
}
