//! The drawn file card: the one picker for a torrent, a KeePass file, a wallpaper, a
//! theme. A path row you can type into, the places beside it, the folder's entries with
//! their size and date, and a name box in save mode. Reading a folder runs off the UI
//! thread: a slow network folder shows "Reading…", never a hang.

use std::{
    path::{Path, PathBuf},
    time::Instant,
};

use serde_json::{Value, json};
use windows::Win32::{
    Foundation::FILETIME,
    Storage::FileSystem::{FileTimeToLocalFileTime, GetDriveTypeW, GetLogicalDrives},
    System::Time::FileTimeToSystemTime,
    UI::{
        Input::KeyboardAndMouse::{SetFocus, VK_DOWN, VK_ESCAPE, VK_RETURN, VK_UP},
        Shell::{
            FOLDERID_Desktop, FOLDERID_Documents, FOLDERID_Downloads, FOLDERID_OneDrive, FOLDERID_Pictures, FOLDERID_Profile, KF_FLAG_DEFAULT,
            SHGetKnownFolderPath,
        },
        WindowsAndMessaging::SetWindowTextW,
    },
};
use windows::core::HSTRING;

use super::{
    Hit, Inner, UiState,
    gfx::{Align, R},
};

/// Which picker this is: what it takes, and what a pick does.
#[derive(Clone, Debug, PartialEq)]
pub enum FileWhat {
    /// A .torrent, read and handed to aria2.
    Torrent,
    /// A .kdbx; the master password is asked next.
    Kdbx,
    /// A wallpaper image.
    Image,
    /// Save this look; the file's text is kept with the card. `suggested` is the name
    /// box's starting text.
    ThemeSave { text: String, suggested: String },
    /// Import a saved look.
    ThemeOpen,
}

impl FileWhat {
    fn title(&self) -> &'static str {
        match self {
            FileWhat::Torrent => "Open a torrent file",
            FileWhat::Kdbx => "Open a KeePass file",
            FileWhat::Image => "Choose a wallpaper",
            FileWhat::ThemeSave { .. } => "Export this look",
            FileWhat::ThemeOpen => "Import a look",
        }
    }
    fn save(&self) -> bool {
        matches!(self, FileWhat::ThemeSave { .. })
    }
    /// Whether a file passes the picker's filter.
    fn takes(&self, name: &str) -> bool {
        let ext = name.rsplit('.').next().unwrap_or("").to_lowercase();
        match self {
            FileWhat::Torrent => ext == "torrent",
            FileWhat::Kdbx => ext == "kdbx",
            FileWhat::Image => matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "bmp" | "gif"),
            FileWhat::ThemeSave { .. } | FileWhat::ThemeOpen => ext == "json",
        }
    }
    /// The folder the card opens at.
    fn start(&self) -> PathBuf {
        let wanted = match self {
            FileWhat::Torrent => FOLDERID_Downloads,
            FileWhat::Kdbx => FOLDERID_Documents,
            FileWhat::Image => FOLDERID_Pictures,
            FileWhat::ThemeSave { .. } | FileWhat::ThemeOpen => FOLDERID_Documents,
        };
        known_folder(&wanted).or_else(|| known_folder(&FOLDERID_Profile)).unwrap_or_else(|| PathBuf::from("C:\\"))
    }
}

/// One row of the folder: its name, and what the columns say.
#[derive(Clone, Debug)]
struct Entry {
    name: String,
    dir: bool,
    size: String,
    date: String,
}

/// One place in the left column: a known folder.
pub struct Place {
    name: &'static str,
    pub(super) dir: PathBuf,
}

pub struct FileCard {
    pub(super) what: FileWhat,
    pub(super) dir: PathBuf,
    /// The folder's entries once a read lands; None while it is being read.
    entries: Option<Vec<Entry>>,
    /// Why a read failed, in words.
    note: Option<String>,
    /// The read the card is waiting for: a late answer for an older folder is dropped.
    seq: u64,
    /// The entry the keyboard or the pointer is on, and the list's scroll.
    sel: usize,
    scroll: f32,
    /// Letters typed: they jump to the first name that starts with them.
    jump: String,
    jump_at: Instant,
    pub(super) places: Vec<Place>,
    /// Filled every frame.
    pub(super) rect: R,
    rows: Vec<R>,
    place_rects: Vec<R>,
    pub(super) drive_rects: Vec<(String, R)>,
    up: R,
    pub(super) path_box: R,
    pub(super) name_box: R,
    go: R,
    cancel: R,
}

const W: f32 = 640.0;
const H: f32 = 520.0;
const PAD: f32 = 14.0;
const ROW_H: f32 = 26.0;
const LEFT_W: f32 = 160.0;

/// Known folders by their Windows ids.
fn known_folder(id: &windows::core::GUID) -> Option<PathBuf> {
    unsafe {
        let p = SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None).ok()?;
        let s = p.to_string().ok();
        windows::Win32::System::Com::CoTaskMemFree(Some(p.0 as _));
        s.map(PathBuf::from)
    }
}

/// The places column: the folders a person actually uses.
fn places() -> Vec<Place> {
    let mut out = Vec::new();
    for (name, id) in [
        ("Home", FOLDERID_Profile),
        ("Desktop", FOLDERID_Desktop),
        ("Downloads", FOLDERID_Downloads),
        ("Documents", FOLDERID_Documents),
        ("Pictures", FOLDERID_Pictures),
        ("OneDrive", FOLDERID_OneDrive),
    ] {
        if let Some(dir) = known_folder(&id) {
            out.push(Place { name, dir });
        }
    }
    out
}

/// The drives Windows has, as "C:" and so on.
fn drives() -> Vec<String> {
    /// DRIVE_NO_ROOT_DIR: the letter has no volume behind it.
    const NO_ROOT: u32 = 1;
    unsafe {
        let mask = GetLogicalDrives();
        let mut out = Vec::new();
        for n in 0..26u32 {
            if mask & (1 << n) == 0 {
                continue;
            }
            let root: Vec<u16> = format!("{}:\\\0", (b'A' + n as u8) as char).encode_utf16().collect();
            if GetDriveTypeW(windows::core::PCWSTR(root.as_ptr())) != NO_ROOT {
                out.push(format!("{}:", (b'A' + n as u8) as char));
            }
        }
        out
    }
}

/// 12 KB, 1.2 MB: a size read at a glance.
fn fmt_size(n: u64) -> String {
    let f = n as f64;
    if n < 1024 {
        format!("{n} B")
    } else if f < 1024.0 * 1024.0 {
        format!("{:.0} KB", f / 1024.0)
    } else if f < 1024.0 * 1024.0 * 1024.0 {
        format!("{:.1} MB", f / (1024.0 * 1024.0))
    } else {
        format!("{:.1} GB", f / (1024.0 * 1024.0 * 1024.0))
    }
}

/// The file's date, in the machine's own time, as YYYY-MM-DD HH:MM.
fn fmt_date(t: std::time::SystemTime) -> String {
    use std::time::UNIX_EPOCH;
    let Ok(since) = t.duration_since(UNIX_EPOCH) else { return String::new() };
    // 1601 to 1970, in seconds; a FILETIME counts 100 ns ticks.
    let ticks = (since.as_secs() + 11_644_473_600) * 10_000_000 + since.subsec_nanos() as u64 / 100;
    let ft = FILETIME { dwLowDateTime: ticks as u32, dwHighDateTime: (ticks >> 32) as u32 };
    unsafe {
        let mut local = FILETIME::default();
        let mut st = windows::Win32::Foundation::SYSTEMTIME::default();
        if FileTimeToLocalFileTime(&ft, &mut local).is_err() || FileTimeToSystemTime(&local, &mut st).is_err() {
            return String::new();
        }
        format!("{:04}-{:02}-{:02} {:02}:{:02}", st.wYear, st.wMonth, st.wDay, st.wHour, st.wMinute)
    }
}

/// Read a folder on a thread of its own; the answer comes back as a "fileList" message.
fn start_read(i: &Inner, dir: &Path, seq: u64) {
    let (proxy, dir) = (i.proxy.clone(), dir.to_path_buf());
    let _ = std::thread::Builder::new().name("lumen-files".into()).spawn(move || {
        let mut list: Vec<Value> = Vec::new();
        let mut note = String::new();
        match std::fs::read_dir(&dir) {
            Ok(rd) => {
                for e in rd.flatten() {
                    let Ok(md) = e.metadata() else { continue };
                    let name = e.file_name().to_string_lossy().to_string();
                    list.push(json!({
                        "n": name,
                        "d": md.is_dir(),
                        "s": if md.is_dir() { String::new() } else { fmt_size(md.len()) },
                        "t": md.modified().map(fmt_date).unwrap_or_default(),
                    }));
                }
            }
            Err(err) => note = format!("Could not read this folder: {err}"),
        }
        let msg = json!({"t": "fileList", "seq": seq, "dir": dir.to_string_lossy(), "list": list, "note": note, "drives": drives()});
        let _ = proxy.send_event(crate::engine::Ev::Ui(msg.to_string()));
    });
}

/// Open the card for one kind of file.
pub fn open(i: &mut Inner, what: FileWhat) {
    let dir = what.start();
    open_at(i, what, dir);
}

/// The same, at a folder of the caller's choosing (the shots use one of Lumen's own).
pub fn open_at(i: &mut Inner, what: FileWhat, dir: PathBuf) {
    i.close_cards();
    let suggested = match &what {
        FileWhat::ThemeSave { suggested, .. } => suggested.clone(),
        _ => String::new(),
    };
    i.files = Some(FileCard {
        what,
        dir: dir.clone(),
        entries: None,
        note: None,
        seq: 1,
        sel: 0,
        scroll: 0.0,
        jump: String::new(),
        jump_at: Instant::now(),
        places: places(),
        rect: R::default(),
        rows: Vec::new(),
        place_rects: Vec::new(),
        drive_rects: Vec::new(),
        up: R::default(),
        path_box: R::default(),
        name_box: R::default(),
        go: R::default(),
        cancel: R::default(),
    });
    unsafe {
        let _ = SetWindowTextW(i.file_path_edit, &HSTRING::from(dir.to_string_lossy().to_string()));
        let _ = SetWindowTextW(i.file_name_edit, &HSTRING::from(suggested.as_str()));
    }
    start_read(i, &dir, 1);
    i.layout_edits();
    unsafe {
        let _ = SetFocus(Some(i.file_path_edit));
    }
    i.invalidate();
}

/// Close without a pick.
pub fn cancel(i: &mut Inner) {
    if i.files.take().is_none() {
        return;
    }
    unsafe {
        if windows::Win32::UI::Input::KeyboardAndMouse::GetFocus() == i.file_path_edit
            || windows::Win32::UI::Input::KeyboardAndMouse::GetFocus() == i.file_name_edit
        {
            let _ = SetFocus(Some(i.hwnd));
        }
    }
    i.layout_edits();
    i.invalidate();
}

/// Show a folder: the places, the drives and Up all land here.
pub fn nav_to(i: &mut Inner, dir: PathBuf) {
    nav(i, dir);
}

/// The card's Open or Save: the picked entry, or whatever the boxes name.
pub fn go(i: &mut Inner) {
    let Some(card) = i.files.as_ref() else { return };
    if card.what.save() {
        let name = super::wide_text(i.file_name_edit).trim().to_string();
        if name.is_empty() {
            return;
        }
        let path = card.dir.join(name);
        pick(i, &path);
        return;
    }
    let typed = PathBuf::from(super::wide_text(i.file_path_edit));
    if typed.is_file() {
        pick(i, &typed);
        return;
    }
    if typed.is_dir() {
        nav(i, typed);
        return;
    }
    let sel = card.sel;
    open_entry(i, sel);
}

/// Show another folder: the read goes off the thread, the list shows "Reading…".
fn nav(i: &mut Inner, dir: PathBuf) {
    let Some(card) = i.files.as_mut() else { return };
    card.dir = dir.clone();
    card.entries = None;
    card.note = None;
    card.sel = 0;
    card.scroll = 0.0;
    card.seq += 1;
    let seq = card.seq;
    unsafe {
        let _ = SetWindowTextW(i.file_path_edit, &HSTRING::from(dir.to_string_lossy().to_string()));
    }
    start_read(i, &dir, seq);
    i.invalidate();
}

/// A read landed from the worker thread. A late answer for another folder is dropped.
pub fn list_landed(i: &mut Inner, m: &Value) {
    let Some(card) = i.files.as_mut() else { return };
    if m["seq"].as_u64() != Some(card.seq) {
        return;
    }
    let mut entries: Vec<Entry> = m["list"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|e| {
                    Some(Entry {
                        name: e["n"].as_str()?.to_string(),
                        dir: e["d"].as_bool().unwrap_or(false),
                        size: e["s"].as_str().unwrap_or("").to_string(),
                        date: e["t"].as_str().unwrap_or("").to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    // Folders first, then by name, as every file list does.
    entries.sort_by(|a, b| b.dir.cmp(&a.dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    card.note = m["note"].as_str().map(|s| s.to_string()).filter(|s| !s.is_empty());
    card.entries = Some(entries);
    i.layout_edits();
    i.invalidate();
}

/// What the card shows, in the order drawn: folders a file never hides behind.
fn shown(card: &FileCard) -> Vec<&Entry> {
    let Some(entries) = &card.entries else { return Vec::new() };
    entries.iter().filter(|e| e.dir || card.what.takes(&e.name)).collect()
}

/// Walk into a folder, or take a file.
fn open_entry(i: &mut Inner, n: usize) {
    let Some(card) = i.files.as_ref() else { return };
    let list = shown(card);
    let Some(e) = list.get(n) else { return };
    let (dir, name) = (card.dir.clone(), e.name.clone());
    if e.dir {
        nav(i, dir.join(name));
        return;
    }
    if !card.what.takes(&name) {
        return;
    }
    pick(i, &dir.join(name));
}

/// Take the file the path or name box names.
fn pick(i: &mut Inner, path: &Path) {
    let Some(card) = i.files.as_ref() else { return };
    let path = path.to_path_buf();
    match &card.what {
        FileWhat::Torrent => {
            let Ok(bytes) = std::fs::read(&path) else {
                if let Some(c) = i.files.as_mut() {
                    c.note = Some("Could not read that file".into());
                }
                i.invalidate();
                return;
            };
            use base64::Engine as _;
            let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
            cancel(i);
            send(&i.proxy, json!({"t": "dl", "a": "addTorrent", "b64": b64}));
        }
        FileWhat::Kdbx => {
            let name = path.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
            cancel(i);
            // The title stays short (the label face is wide); the box says which file.
            super::form::open_fields(
                i,
                "KeePass vault",
                &[(&format!("Master password for {name}"), "", true)],
                super::form::FormAct::VaultOpen { path: path.to_string_lossy().to_string() },
            );
        }
        FileWhat::Image => {
            cancel(i);
            super::pages::set(i, "wallpaper", json!(path.to_string_lossy()));
        }
        FileWhat::ThemeSave { text, .. } => {
            let text = text.clone();
            if path.exists() {
                // Already there: the question, drawn by the confirm card, over this one.
                super::confirm::open_over(i, super::confirm::ConfirmKind::Replace { path: path.to_string_lossy().to_string(), text });
            } else {
                write_theme(i, &path, &text);
            }
        }
        FileWhat::ThemeOpen => {
            let Some(t) = std::fs::read_to_string(&path).ok().and_then(|t| super::style::ThemeFile::from_json(&t)) else {
                if let Some(c) = i.files.as_mut() {
                    c.note = Some("That file is not a Lumen theme".into());
                }
                i.invalidate();
                return;
            };
            cancel(i);
            super::pages::apply_theme(i, t);
        }
    }
}

/// Write the exported look, and say so.
pub fn write_theme(i: &mut Inner, path: &Path, text: &str) {
    let note = match std::fs::write(path, text) {
        Ok(()) => format!("Exported to {}", path.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default()),
        Err(_) => "Could not write that file".to_string(),
    };
    cancel(i);
    i.page.flash = Some((note, Instant::now()));
    i.invalidate();
}

fn send(proxy: &tao::event_loop::EventLoopProxy<crate::engine::Ev>, v: Value) {
    let _ = proxy.send_event(crate::engine::Ev::Ui(v.to_string()));
}

/// Escape closes, Enter opens the picked entry or takes it, Up and Down move.
pub fn key(i: &mut Inner, vk: u32) -> bool {
    if i.files.is_none() {
        return false;
    }
    match vk {
        v if v == VK_ESCAPE.0 as u32 => {
            cancel(i);
            true
        }
        v if v == VK_RETURN.0 as u32 => {
            let sel = i.files.as_ref().map(|c| c.sel).unwrap_or(0);
            open_entry(i, sel);
            true
        }
        v if v == VK_DOWN.0 as u32 || v == VK_UP.0 as u32 => {
            let n = i.files.as_ref().map(|c| shown(c).len()).unwrap_or(0);
            if n > 0 {
                let step: isize = if v == VK_DOWN.0 as u32 { 1 } else { -1 };
                if let Some(c) = i.files.as_mut() {
                    c.sel = (c.sel as isize + step).rem_euclid(n as isize) as usize;
                    c.scroll_to();
                }
                i.invalidate();
            }
            true
        }
        _ => false,
    }
}

/// A letter typed: it jumps to the first name that starts with what is typed so far.
pub fn typed_char(i: &mut Inner, ch: u32) {
    let Some(card) = i.files.as_mut() else { return };
    let Some(c) = char::from_u32(ch) else { return };
    if c.is_control() {
        return;
    }
    if card.jump_at.elapsed().as_secs_f32() > 1.2 {
        card.jump.clear();
    }
    card.jump.push(c.to_ascii_lowercase());
    card.jump_at = Instant::now();
    let jump = card.jump.clone();
    let at = shown(card).iter().position(|e| e.name.to_lowercase().starts_with(&jump));
    if let Some(n) = at {
        card.sel = n;
        card.scroll_to();
    }
    i.invalidate();
}

/// The wheel over the list: the card scrolls, not the page under it.
pub fn wheel(i: &mut Inner, x: f32, y: f32, delta: i32) -> bool {
    let Some(card) = i.files.as_ref() else { return false };
    if !card.rect.contains(x, y) {
        return false;
    }
    let cols_top = card.path_box.bottom() + 8.0 + 18.0;
    let list_bottom = if card.what.save() { card.name_box.y - 10.0 } else { card.go.y - 10.0 };
    let list_h = list_bottom - cols_top;
    let n = shown(card).len() as f32;
    let max = (n * ROW_H - list_h).max(0.0);
    if let Some(c) = i.files.as_mut() {
        c.scroll = (c.scroll - delta as f32 / 120.0 * ROW_H * 2.0).clamp(0.0, max);
    }
    i.invalidate();
    true
}

/// The pointer came up: on a row it may open or take it, anywhere else closes the card.
/// A question card over this one owns the click while it is open.
pub fn clicked_outside(i: &mut Inner, h: &Option<Hit>) {
    if i.files.is_none() || i.confirm.is_some() {
        return;
    }
    if !matches!(
        h,
        Some(
            Hit::FileCard
                | Hit::FileRow(_)
                | Hit::FilePlace(_)
                | Hit::FileDrive(_)
                | Hit::FileUp
                | Hit::FileGo
                | Hit::FileCancel
                | Hit::FilePath
                | Hit::FileName
        )
    ) {
        cancel(i);
    }
}

/// A row was clicked: the first click picks it; a second, or Enter, opens it.
pub fn click_row(i: &mut Inner, n: usize) {
    let Some(card) = i.files.as_ref() else { return };
    let was = card.sel;
    if let Some(c) = i.files.as_mut() {
        c.sel = n;
    }
    if was == n {
        open_entry(i, n);
    }
    i.invalidate();
}

/// Where the card and its parts sit: the middle of the window, so the list has room.
fn geometry(i: &Inner, card: &FileCard) -> (R, Vec<R>, Vec<R>, Vec<(String, R)>, R, R, R, R, R) {
    let (w_win, h_win) = i.size_dip();
    let x = ((w_win - W) / 2.0).clamp(8.0, (w_win - W - 8.0).max(8.0));
    let y = ((h_win - H) / 2.0).clamp(8.0, (h_win - H - 8.0).max(8.0));
    let rect = R::new(x, y, W, H);
    let path_box = R::new(rect.x + PAD, rect.y + PAD + 14.0 + 10.0, W - 2.0 * PAD - 52.0, 30.0);
    let up = R::new(path_box.right() + 8.0, path_box.y, 44.0, 30.0);
    // The label band sits right under the path row; the columns start under it.
    let label_y = path_box.bottom() + 8.0;
    let cols_top = label_y + 18.0;
    let cancel = R::new(rect.right() - PAD - 84.0, rect.bottom() - PAD - 28.0, 84.0, 28.0);
    let go = R::new(cancel.x - 8.0 - 84.0, cancel.y, 84.0, 28.0);
    let name_box = if card.what.save() {
        R::new(rect.x + PAD + LEFT_W + 12.0, go.y - 8.0 - 26.0, rect.right() - PAD - (rect.x + PAD + LEFT_W + 12.0), 26.0)
    } else {
        R::default()
    };
    let list_bottom = if card.what.save() { name_box.y - 10.0 } else { go.y - 10.0 };
    let list_x = rect.x + PAD + LEFT_W + 12.0;
    let list_w = rect.right() - PAD - list_x;
    let mut rows = Vec::new();
    let mut ry = cols_top;
    while ry + ROW_H <= list_bottom {
        rows.push(R::new(list_x, ry, list_w, ROW_H));
        ry += ROW_H;
    }
    let mut place_rects = Vec::new();
    let mut py = cols_top;
    for _ in 0..card.places.len() {
        place_rects.push(R::new(rect.x + PAD, py, LEFT_W, 24.0));
        py += 26.0;
    }
    let mut drive_rects = Vec::new();
    let mut dx = rect.x + PAD;
    // Room for the "Drives" label, as the columns leave room for theirs.
    py += 8.0 + 18.0;
    for name in drives() {
        if dx + 44.0 > rect.x + PAD + LEFT_W {
            dx = rect.x + PAD;
            py += 28.0;
        }
        drive_rects.push((name, R::new(dx, py, 42.0, 24.0)));
        dx += 46.0;
    }
    (rect, rows, place_rects, drive_rects, up, path_box, name_box, go, cancel)
}

impl FileCard {
    /// Keep the picked row inside the drawn window.
    fn scroll_to(&mut self) {
        let rows = self.rows.len() as f32;
        let at = self.sel as f32 * ROW_H;
        if at < self.scroll {
            self.scroll = at;
        } else if at + ROW_H > self.scroll + rows * ROW_H {
            self.scroll = at + ROW_H - rows * ROW_H;
        }
    }
}

/// Draw the card: title, path row, places, drives, the entries, the buttons.
pub fn draw(i: &mut Inner, _s: &UiState) {
    let Some(mut card) = i.files.take() else { return };
    let lo = i.look.clone();
    let th = lo.theme;
    let (rect, rows, place_rects, drive_rects, up, path_box, name_box, go, cancel) = geometry(i, &card);
    card.rect = rect;
    card.rows = rows;
    card.place_rects = place_rects;
    card.drive_rects = drive_rects;
    card.up = up;
    card.path_box = path_box;
    card.name_box = name_box;
    card.go = go;
    card.cancel = cancel;
    i.float_card(rect, 1.0);
    i.hit(rect, Hit::FileCard);
    i.label(card.what.title(), R::new(rect.x + PAD, rect.y + PAD + 1.0, W - 2.0 * PAD, 14.0), th.faint);

    // The path row: the box, its value drawn under the edit child, and Up.
    i.gfx.round(path_box, 6.0, th.bg);
    i.gfx.round_stroke(path_box, 6.0, th.line, 1.0);
    let path = card.dir.to_string_lossy().to_string();
    i.gfx.text(&path, R::new(path_box.x + 8.0, path_box.y, path_box.w - 16.0, path_box.h), lo.font(12.5, 450), th.text);
    i.hit(path_box, Hit::FilePath);
    let hov = i.is_hover(&Hit::FileUp);
    i.gfx.round(up, 6.0, if hov { th.hover } else { th.raise });
    i.gfx.round_stroke(up, 6.0, th.line, 1.0);
    i.gfx.text("Up", up, lo.font(12.0, 500).align(Align::Center), if hov { th.text } else { th.dim });
    i.hit(up, Hit::FileUp);

    // The places, and the drives under them.
    i.label("Places", R::new(rect.x + PAD, path_box.bottom() + 8.0, LEFT_W, 12.0), th.faint);
    for (n, (p, r)) in card.places.iter().zip(card.place_rects.iter()).enumerate() {
        let here = card.dir == p.dir;
        let hov = i.is_hover(&Hit::FilePlace(n));
        if here || hov {
            i.gfx.round(*r, lo.r_sm, if here { th.pill } else { th.hover });
        }
        let ink = if here { th.pill_ink } else { th.dim };
        i.gfx.text(p.name, R::new(r.x + 8.0, r.y, r.w - 16.0, r.h), lo.font(12.5, 500), ink);
        i.hit(*r, Hit::FilePlace(n));
    }
    let drives_y = card.place_rects.last().map(|r| r.bottom() + 10.0).unwrap_or(rect.y + 200.0);
    i.label("Drives", R::new(rect.x + PAD, drives_y, LEFT_W, 12.0), th.faint);
    for (n, (name, r)) in card.drive_rects.iter().enumerate() {
        let here = card.dir.to_string_lossy().starts_with(&format!("{name}\\"));
        let hov = i.is_hover(&Hit::FileDrive(n));
        if here || hov {
            i.gfx.round(*r, lo.r_sm, if here { th.pill } else { th.hover });
        }
        let ink = if here { th.pill_ink } else { th.dim };
        i.gfx.text(name, *r, lo.font(12.0, 500).align(Align::Center), ink);
        i.hit(*r, Hit::FileDrive(n));
    }

    // The folder's entries: folders first, a size and a date beside each.
    let list = shown(&card);
    match &card.entries {
        None => {
            if let Some(r) = card.rows.first() {
                i.gfx.text("Reading…", R::new(r.x + 8.0, r.y, r.w, r.h), lo.font(12.5, 450), th.faint);
            }
        }
        Some(_) => {
            let list_h = card.rows.len() as f32 * ROW_H;
            let max = (list.len() as f32 * ROW_H - list_h).max(0.0);
            let scroll = card.scroll.clamp(0.0, max);
            i.gfx.clip(R::new(card.rows.first().map(|r| r.x).unwrap_or(0.0), card.rows.first().map(|r| r.y).unwrap_or(0.0), card.rows.first().map(|r| r.w).unwrap_or(0.0), list_h));
            for (n, e) in list.iter().enumerate() {
                let top = card.rows.first().map(|r| r.y).unwrap_or(0.0) + n as f32 * ROW_H - scroll;
                let r = R::new(card.rows.first().map(|r| r.x).unwrap_or(0.0), top, card.rows.first().map(|r| r.w).unwrap_or(0.0), ROW_H);
                let picked = n == card.sel;
                let hov = i.is_hover(&Hit::FileRow(n));
                if picked || hov {
                    i.gfx.round(r, lo.r_sm, if picked { th.hover } else { th.hover.alpha(0.6) });
                }
                let ink = if e.dir { th.text } else { th.dim };
                i.gfx.text(&e.name, R::new(r.x + 8.0, r.y, r.w - 260.0, r.h), lo.font(12.5, if e.dir { 550 } else { 450 }), ink);
                i.gfx.text(&e.size, R::new(r.right() - 250.0, r.y, 70.0, r.h), lo.font(11.5, 450).align(Align::Right), th.faint);
                i.gfx.text(&e.date, R::new(r.right() - 170.0, r.y, 160.0, r.h), lo.font(11.5, 450).align(Align::Right), th.faint);
                i.hit(r, Hit::FileRow(n));
            }
            i.gfx.unclip();
            if list.is_empty() {
                if let Some(r) = card.rows.first() {
                    i.gfx.text("Nothing here fits", R::new(r.x + 8.0, r.y, r.w, r.h), lo.font(12.5, 450), th.faint);
                }
            }
        }
    }
    if let Some(note) = &card.note {
        if let Some(r) = card.rows.last() {
            i.gfx.text(note, R::new(r.x + 8.0, r.y, r.w, r.h), lo.font(12.0, 450), th.danger);
        }
    }

    // Save mode: the name box, its value drawn under its edit child.
    if card.what.save() {
        let name = super::wide_text(i.file_name_edit);
        i.gfx.round(name_box, 6.0, th.bg);
        i.gfx.round_stroke(name_box, 6.0, th.line, 1.0);
        i.gfx.text(&name, R::new(name_box.x + 8.0, name_box.y, name_box.w - 16.0, name_box.h), lo.font(12.5, 450), th.text);
        i.hit(name_box, Hit::FileName);
    }
    let go_label = if card.what.save() { "Save" } else { "Open" };
    i.pill_button(go, go_label, Hit::FileGo, true, 1.0);
    i.pill_button(cancel, "Cancel", Hit::FileCancel, false, 1.0);
    i.files = Some(card);
}
