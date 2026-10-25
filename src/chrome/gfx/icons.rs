//! The icon set: Lucide (https://lucide.dev, ISC), embedded as SVG text.

use windows::{
    Win32::{
        Foundation::HMODULE,
        Graphics::{
            Direct2D::{Common::*, *},
            Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP},
            Direct3D11::*,
            Dxgi::{Common::DXGI_FORMAT_B8G8R8A8_UNORM, IDXGIDevice},
        },
        UI::Shell::SHCreateMemStream,
    },
    core::Interface,
};

pub struct Entry {
    pub name: &'static str,
    svg: &'static str,
    filled: bool,
    pub scale: f32,
}

macro_rules! lucide {
    ($name:literal, $file:literal) => {
        lucide!($name, $file, false, 1.0)
    };
    ($name:literal, $file:literal, filled) => {
        lucide!($name, $file, true, 1.0)
    };
    ($name:literal, $file:literal, scale $k:literal) => {
        lucide!($name, $file, false, $k)
    };
    ($name:literal, $file:literal, $filled:literal, $k:literal) => {
        Entry {
            name: $name,
            svg: include_str!(concat!("../../../assets/icons/lucide/", $file, ".svg")),
            filled: $filled,
            scale: $k,
        }
    };
}

pub static TABLE: &[Entry] = &[
    lucide!("back", "arrow-left"),
    lucide!("fwd", "arrow-right"),
    lucide!("reload", "rotate-cw"),
    lucide!("stop", "x"),
    lucide!("close", "x"),
    lucide!("min", "minus"),
    lucide!("max", "square", scale 0.78),
    lucide!("restore", "copy", scale 0.72),
    lucide!("plus", "plus"),
    lucide!("search", "search"),
    lucide!("lock", "lock"),
    lucide!("globe", "globe"),
    lucide!("tab", "app-window"),
    lucide!("sliders", "sliders-horizontal"),
    lucide!("info", "info"),
    lucide!("mic", "mic"),
    lucide!("leaf", "leaf"),
    lucide!("external", "external-link"),
    lucide!("sound", "volume-2"),
    lucide!("muted", "volume-x"),
    lucide!("star", "star"),
    lucide!("star-on", "star", filled),
    lucide!("clock", "clock"),
    lucide!("history", "rotate-ccw-clock"),
    lucide!("download", "download"),
    lucide!("pin", "pin"),
    lucide!("bookmark", "bookmark"),
    lucide!("shield", "shield"),
    lucide!("dashboard", "layout-dashboard"),
    lucide!("palette", "palette"),
    lucide!("type", "type"),
    lucide!("grid", "layout-grid"),
    lucide!("zap", "zap"),
    lucide!("key", "key-round"),
    lucide!("bot", "bot"),
    lucide!("languages", "languages"),
    lucide!("keyboard", "keyboard"),
    lucide!("shapes", "shapes"),
    lucide!("wind", "wind"),
    lucide!("play", "play", filled),
    lucide!("pause", "pause", filled),
    lucide!("pip", "picture-in-picture"),
    lucide!("next", "skip-forward"),
    lucide!("sidebar", "panel-left"),
    lucide!("split", "columns-2"),
    lucide!("send", "send"),
    lucide!("home", "house"),
    lucide!("mask", "venetian-mask"),
    lucide!("cookie", "cookie"),
    lucide!("eraser", "eraser"),
    lucide!("unlink", "link-2-off"),
    lucide!("redirect", "route-off"),
    lucide!("danger", "shield-alert"),
    lucide!("https", "shield-check"),
    lucide!("check", "check"),
    lucide!("alert", "triangle-alert"),
];

#[cfg(test)]
pub static GEOMETRY: &[&str] = &[
    "back", "fwd", "reload", "stop", "close", "min", "max", "restore", "plus", "search", "lock", "globe", "tab", "sliders",
    "sound", "muted", "star", "star-on", "clock", "download", "pin", "play", "pause", "sidebar", "split", "home",
];

pub fn find(name: &str) -> Option<(usize, &'static Entry)> {
    TABLE.iter().enumerate().find(|(_, e)| e.name == name)
}

#[cfg(test)]
pub fn known(name: &str) -> bool {
    find(name).is_some() || GEOMETRY.contains(&name)
}

pub fn stroke24(dip: f32) -> f32 {
    if dip >= 20.0 {
        1.5
    } else if dip >= 15.0 {
        1.75
    } else if dip >= 11.0 {
        2.0
    } else {
        2.25
    }
}

impl Entry {
    pub fn source(&self, stroke: f32) -> String {
        let s = self.svg.replace("stroke-width=\"2\"", &format!("stroke-width=\"{stroke}\""));
        let s = if self.filled { s.replacen("fill=\"none\"", "fill=\"currentColor\"", 1) } else { s };
        s.replace("currentColor", "#fff")
    }
}

pub struct Rasterizer {
    ctx: ID2D1DeviceContext5,
}

impl Rasterizer {
    pub fn new(factory: &ID2D1Factory) -> Option<Rasterizer> {
        unsafe {
            let mut dev: Option<ID3D11Device> = None;
            for kind in [D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP] {
                let made = D3D11CreateDevice(
                    None,
                    kind,
                    HMODULE::default(),
                    D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                    None,
                    D3D11_SDK_VERSION,
                    Some(&mut dev),
                    None,
                    None,
                );
                if made.is_ok() && dev.is_some() {
                    break;
                }
            }
            let dxgi: IDXGIDevice = dev?.cast().ok()?;
            let f1: ID2D1Factory1 = factory.cast().ok()?;
            let d = f1.CreateDevice(&dxgi).ok()?;
            let ctx = d.CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE).ok()?;
            Some(Rasterizer { ctx: ctx.cast().ok()? })
        }
    }

    pub fn alpha(&self, svg: &str, px: u32) -> Option<Vec<u8>> {
        unsafe {
            let c = &self.ctx;
            let stream = SHCreateMemStream(Some(svg.as_bytes()))?;
            let doc = c.CreateSvgDocument(&stream, D2D_SIZE_F { width: 24.0, height: 24.0 }).ok()?;
            let size = D2D_SIZE_U { width: px, height: px };
            let fmt = D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED };
            let draw = c
                .CreateBitmap(
                    size,
                    None,
                    0,
                    &D2D1_BITMAP_PROPERTIES1 {
                        pixelFormat: fmt,
                        dpiX: 96.0,
                        dpiY: 96.0,
                        bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET,
                        colorContext: std::mem::ManuallyDrop::new(None),
                    },
                )
                .ok()?;
            c.SetTarget(&draw);
            c.SetDpi(96.0, 96.0);
            c.BeginDraw();
            c.Clear(Some(&D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 0.0 }));
            let k = px as f32 / 24.0;
            c.SetTransform(&windows_numerics::Matrix3x2 { M11: k, M12: 0.0, M21: 0.0, M22: k, M31: 0.0, M32: 0.0 });
            c.DrawSvgDocument(&doc);
            c.EndDraw(None, None).ok()?;
            c.SetTarget(None::<&ID2D1Image>);
            let read = c
                .CreateBitmap(
                    size,
                    None,
                    0,
                    &D2D1_BITMAP_PROPERTIES1 {
                        pixelFormat: fmt,
                        dpiX: 96.0,
                        dpiY: 96.0,
                        bitmapOptions: D2D1_BITMAP_OPTIONS_CPU_READ | D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
                        colorContext: std::mem::ManuallyDrop::new(None),
                    },
                )
                .ok()?;
            read.CopyFromBitmap(None, &draw, None).ok()?;
            let m = read.Map(D2D1_MAP_OPTIONS_READ).ok()?;
            let mut out = vec![0u8; (px * px) as usize];
            for y in 0..px as usize {
                let row = std::slice::from_raw_parts(m.bits.add(y * m.pitch as usize), px as usize * 4);
                for x in 0..px as usize {
                    out[y * px as usize + x] = row[x * 4 + 3];
                }
            }
            let _ = read.Unmap();
            Some(out)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names_used(src: &str) -> Vec<String> {
        let mut out = Vec::new();
        for line in src.lines() {
            let t = line.trim_start();
            if t.starts_with("//") {
                continue;
            }
            if t.contains("json!") {
                continue;
            }
            let call = ["gfx.icon(", ".button(", ".chip(", "empty_state(", "icon(", "Some(\""].iter().any(|k| line.contains(k));
            let tuple = (t.starts_with("(\"") || t.starts_with("(if ")) && t.contains("Hit::") || t.starts_with("SuggKind::");
            if !(call || tuple) {
                continue;
            }
            let mut rest = line;
            while let Some(a) = rest.find('"') {
                let Some(b) = rest[a + 1..].find('"') else { break };
                let lit = &rest[a + 1..a + 1 + b];
                if !lit.is_empty() && lit.bytes().all(|c| c.is_ascii_lowercase() || c == b'-') {
                    out.push(lit.to_string());
                }
                rest = &rest[a + b + 2..];
            }
        }
        out
    }

    const NOT_ICONS: &[&str] = &["playing"];

    #[test]
    fn every_icon_name_in_the_chrome_has_a_drawing() {
        let mut bad = Vec::new();
        let mut seen = 0;
        for (file, src) in [
            ("mod.rs", include_str!("../mod.rs")),
            ("pages.rs", include_str!("../pages.rs")),
            ("doctor.rs", include_str!("../doctor.rs")),
        ] {
            for n in names_used(src) {
                if known(&n) {
                    seen += 1;
                } else if !NOT_ICONS.contains(&n.as_str()) {
                    bad.push(format!("{file}: {n}"));
                }
            }
        }
        assert!(seen > 20, "scan found only {seen} icon names");
        assert!(bad.is_empty(), "names with no SVG and no geometry: {bad:?}");
    }

    #[test]
    fn the_scan_catches_a_missing_icon() {
        let src = "self.gfx.icon(\"frobnicate\", x, y, 16.0, c);
self.button(r, \"reload\", Hit::Reload, true);";
        let used = names_used(src);
        assert_eq!(used, ["frobnicate", "reload"]);
        assert!(!known("frobnicate") && known("reload"));
    }

    #[test]
    fn table_is_sound() {
        for (i, e) in TABLE.iter().enumerate() {
            assert!(e.svg.contains("<svg") && e.svg.contains("stroke-width=\"2\""), "{} is not a Lucide SVG", e.name);
            assert!(TABLE[..i].iter().all(|o| o.name != e.name), "{} listed twice", e.name);
            assert!(e.scale > 0.0 && e.scale <= 1.0);
        }
        assert!(GEOMETRY.iter().all(|g| find(g).is_some()), "a geometry name Lucide lacks should get its own row");
    }

    #[test]
    fn source_recolors_and_restrokes() {
        let (_, star) = find("star-on").unwrap();
        let s = star.source(1.75);
        assert!(s.contains("stroke-width=\"1.75\"") && !s.contains("currentColor") && s.contains("fill=\"#fff\""));
        let (_, line) = find("star").unwrap();
        assert!(line.source(2.0).contains("fill=\"none\""));
    }

    #[test]
    fn strokes_thin_out_as_icons_grow() {
        assert!(stroke24(10.0) >= stroke24(16.0) && stroke24(16.0) >= stroke24(24.0));
    }
}
