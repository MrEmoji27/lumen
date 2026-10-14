// Bake the icon into the .exe. Explorer, the taskbar and Alt+Tab all read it from here.
fn main() {
    println!("cargo:rerun-if-changed=lumen.rc");
    println!("cargo:rerun-if-changed=assets/lumen.ico");
    embed_resource::compile("lumen.rc", embed_resource::NONE)
        .manifest_optional()
        .unwrap();
}
