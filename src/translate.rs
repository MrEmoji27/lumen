//! Offline page translation: Mozilla's Bergamot, Marian NMT compiled to WebAssembly.
//!
//! Nothing leaves the machine. The engine is bundled (MPL-2.0, assets/bergamot/)
//! and written under data_dir()/bergamot/ on first use; the model for a language
//! pair is fetched once from Mozilla's model bucket into data_dir()/models/<pair>/
//! and checked against the SHA-256 the registry publishes.
//!
//! The wasm does not run in the page being read: a page's own CSP may forbid
//! workers and WebAssembly, and its text would have to be handed over anyway.
//! A hidden WebView2 of Lumen's own does the work instead. It loads Lumen's page
//! over a host name nothing resolves, mapped to data_dir(), so the engine and the
//! model files are read from disk; it is dropped as soon as the texts come back,
//! which takes the worker and its model memory with it.

use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    rc::Rc,
    time::Duration,
};

use sha2::{Digest, Sha256};
use tao::dpi::{LogicalPosition, LogicalSize};
use webview2_com::{Microsoft::Web::WebView2::Win32::*, WebMessageReceivedEventHandler};
use windows::core::{HSTRING, Interface, PWSTR};
use wry::{Rect, WebView, WebViewExtWindows};

use crate::engine::{Ctx, Ev};

/// The Bergamot engine, as Firefox Translations 1.3.4 ships it (bergamot-translator
/// v0.4.4, MPL-2.0, assets/bergamot/LICENSE). Both files are pinned here; a change
/// in the assets folder is caught by `bundled_engine_matches_its_pins`.
const GLUE: &str = include_str!("../assets/bergamot/bergamot-translator-worker.js");
const WASM: &[u8] = include_bytes!("../assets/bergamot/bergamot-translator-worker.wasm");
const GLUE_SHA256: &str = "8dbe289945fc7f175d48385bc7cdef7122ef6073e77c4355d8cd724da365834c";
const WASM_SHA256: &str = "dad40ec05e6e26f6df56aa37f0d3a208adcd5d35d3347e0f9a8a267c89d4f947";

/// Where the registry says the models are. HTTPS, and every file is checked
/// against the registry before it is kept.
const MODELS_ROOT: &str = "https://storage.googleapis.com/bergamot-models-sandbox/0.3.4";
/// The host name data_dir() answers on inside the translator page. ".invalid" is
/// reserved and never resolves, so a failed mapping cannot reach a real server.
const HOST: &str = "lumen-data.invalid";
/// The biggest model in the registry is 17 MB. Twice that is not a model.
const MAX_FETCH: u64 = 64 << 20;
/// One pair's model download, from a connection that may be slow.
const FETCH_TIMEOUT: Duration = Duration::from_secs(600);

/// One file of a language pair, as Firefox Translations' registry lists it.
pub struct M {
    pub name: &'static str,
    pub bytes: u64,
    pub sha256: &'static str,
}

/// A language pair Lumen can translate: `src` into `dst`, two-letter codes, and the
/// three files a Bergamot model is. Names, sizes and hashes are from the registry
/// (model/modelRegistry.js of the shipped add-on, MPL-2.0).
pub struct P {
    pub src: &'static str,
    pub dst: &'static str,
    pub files: [M; 3],
}

impl P {
    /// The pair's name the way the registry and the engine call it: "esen".
    pub fn name(&self) -> String {
        format!("{}{}", self.src, self.dst)
    }
}

/// Every pair the registry carries with English on one side. Generated from
/// modelRegistry.js; the pairs the registry has no complete entry for are left out.
const PAIRS: &[P] = &[
    P { src: "bg", dst: "en", files: [
        M { name: "model.bgen.intgemm.alphas.bin", bytes: 17140899, sha256: "71900847a98cf66bd1d05eaafc23a794c8c1285fb3f0e2ecd2849e6f81c79d53" },
        M { name: "lex.50.50.bgen.s2t.bin", bytes: 6182512, sha256: "71e8d040a2f63705bec232cd186f32e9f9a78e7968216516c4535589f6a828f9" },
        M { name: "vocab.bgen.spm", bytes: 920621, sha256: "24ce87ba39216714f222ca6a105f30b1863a7ef8b58c9fafdc7a66184e9813a5" },
    ] },
    P { src: "ca", dst: "en", files: [
        M { name: "model.caen.intgemm.alphas.bin", bytes: 17140899, sha256: "3a315266490d87f72adf9e5387ee567b2fb76a30018e51586b882b1d87bf5aed" },
        M { name: "lex.50.50.caen.s2t.bin", bytes: 5244644, sha256: "a648be17d6f008feee687b455d00dbfaedba2ead8bee32658783c4325a8d3ece" },
        M { name: "vocab.caen.spm", bytes: 811443, sha256: "10a1f25e5640f596b547190082f87ba4994f8714693904c82a35d965b9cc7470" },
    ] },
    P { src: "cs", dst: "en", files: [
        M { name: "model.csen.intgemm.alphas.bin", bytes: 17140756, sha256: "5b16661e2864dc50b2f4091a16bdd4ec8d8283e04271e602159ba348df5d6e2d" },
        M { name: "lex.50.50.csen.s2t.bin", bytes: 4535788, sha256: "8228a3c3f7887759a62b7d7c674a7bef9b70161913f9b0939ab58f71186835c2" },
        M { name: "vocab.csen.spm", bytes: 769763, sha256: "f71cc5d045e479607078e079884f44032f5a0b82547fb96eefa29cd1eb47c6f3" },
    ] },
    P { src: "de", dst: "en", files: [
        M { name: "model.deen.intgemm.alphas.bin", bytes: 17140837, sha256: "1980225d00dc5645491777accff5b3c9d20b92eff67a25135f1cf8fe2ed8fb8f" },
        M { name: "lex.50.50.deen.s2t.bin", bytes: 5047568, sha256: "2f7c0f7bbce97ae5b52454074a892ba7b7610fb98e3c5d341e4ca79f0850c4de" },
        M { name: "vocab.deen.spm", bytes: 784269, sha256: "417668f2ed297970febafb5b079a9d5ebc4ed0b3550ac8386d67a90473a09bd7" },
    ] },
    P { src: "es", dst: "en", files: [
        M { name: "model.esen.intgemm.alphas.bin", bytes: 17140755, sha256: "4b6b7f451094aaa447d012658af158ffc708fc8842dde2f871a58404f5457fe0" },
        M { name: "lex.50.50.esen.s2t.bin", bytes: 3860888, sha256: "f11a2c23ef85ab1fee1c412b908d69bc20d66fd59faa8f7da5a5f0347eddf969" },
        M { name: "vocab.esen.spm", bytes: 825463, sha256: "909b1eea1face0d7f90a474fe29a8c0fef8d104b6e41e65616f864c964ba8845" },
    ] },
    P { src: "et", dst: "en", files: [
        M { name: "model.eten.intgemm.alphas.bin", bytes: 17140754, sha256: "aac98a2371e216ee2d4843cbe896c617f6687501e17225ac83482eba52fd0028" },
        M { name: "lex.50.50.eten.s2t.bin", bytes: 3974944, sha256: "6992bedc590e60e610a28129c80746fe5f33144a4520e2c5508d87db14ca54f8" },
        M { name: "vocab.eten.spm", bytes: 828426, sha256: "e3b66bc141f6123cd40746e2fb9b8ee4f89cbf324ab27d6bbf3782e52f15fa2d" },
    ] },
    P { src: "fa", dst: "en", files: [
        M { name: "model.faen.intgemm.alphas.bin", bytes: 17140837, sha256: "f7a76cde1d66aa7d9ed017d7dd0f7e945608412e723e0d99dee293a6adae4572" },
        M { name: "lex.50.50.faen.s2t.bin", bytes: 6197320, sha256: "55fac2186b0c8c903d4ed958457f363b3a339a96b8ab7f73b62c7b7c9ac09b48" },
        M { name: "vocab.faen.spm", bytes: 845020, sha256: "c3a675794dcba12dde8e1a343a82d973b7c9584f9dce3dad37f99dfe64fd29de" },
    ] },
    P { src: "fr", dst: "en", files: [
        M { name: "model.fren.intgemm.alphas.bin", bytes: 17140961, sha256: "185f76d24c2d400fe4ea0cb2487df77722641b97a3ef10633872e8a7fdf40e09" },
        M { name: "lex.50.50.fren.s2t.bin", bytes: 8818768, sha256: "3148abf21ea98a4d69d0e4504e0d68a6c060204a9b9a39b76855aee1d5b2c8ea" },
        M { name: "vocab.fren.spm", bytes: 831382, sha256: "4c84b95b62c930f0791466d73eb996841eef474c96d0c2f0e6c6d80640f2005a" },
    ] },
    P { src: "is", dst: "en", files: [
        M { name: "model.isen.intgemm.alphas.bin", bytes: 17140780, sha256: "29ff1cc6072372ee41f91d38dbf1cdfa4efb1a89a0c0c4200a443ee929e091b7" },
        M { name: "lex.50.50.isen.s2t.bin", bytes: 2926468, sha256: "790a9dd90e07b105437df60d79cb30fcb34fdce918b4aa5e94992953bcc42520" },
        M { name: "vocab.isen.spm", bytes: 820367, sha256: "67020a6ce013f06a1fca8b1f4c1a65c4fb8c679b231a236e7c6988570cbc8e06" },
    ] },
    P { src: "it", dst: "en", files: [
        M { name: "model.iten.intgemm.alphas.bin", bytes: 17140899, sha256: "7dfdf189146d9353fdea264b9e4c8ac36441c770dc4353a8380b64e589dc035b" },
        M { name: "lex.50.50.iten.s2t.bin", bytes: 4977500, sha256: "e30ec549bd0da9ac42cccdcd3806d3be84d485f7fd329f90f6e40ee027e841d9" },
        M { name: "vocab.iten.spm", bytes: 812781, sha256: "603f3349657c3deb9736a0c567452d102a5a03c377dfdf1d32c428608f2cff1b" },
    ] },
    P { src: "nb", dst: "en", files: [
        M { name: "model.nben.intgemm.alphas.bin", bytes: 12980780, sha256: "03cef6aa909d2397420201551354c21010324fe155753d62b4eb190adca071be" },
        M { name: "lex.50.50.nben.s2t.bin", bytes: 1487760, sha256: "5ec9a1eb849cf8fde0908904808e65a0c5a3027f85b1b5b9944c6b15d424598d" },
        M { name: "vocab.nben.spm", bytes: 511372, sha256: "3cb4f4efc382d6e7242304e8649050da297469b69a604dd7480d8704fe4877b6" },
    ] },
    P { src: "nl", dst: "en", files: [
        M { name: "model.nlen.intgemm.alphas.bin", bytes: 17140899, sha256: "187f9c8f1ffbcc8ecf0724b608ffed9f15bde262b9de5c4754f8d763b585969d" },
        M { name: "lex.50.50.nlen.s2t.bin", bytes: 4940304, sha256: "15c473e6e5aef1fa3fe5cc804844e19a2c671d2e39169dcbc06f456ca4f9b2c0" },
        M { name: "vocab.nlen.spm", bytes: 807730, sha256: "48a6331cb8a69d21dd24c6c2a15de44a07bcfd4a4470eaf4616849d77945f6b9" },
    ] },
    P { src: "nn", dst: "en", files: [
        M { name: "model.nnen.intgemm.alphas.bin", bytes: 12980780, sha256: "17550943381bdfae30b1d8ee29b33b2da2c9ae4d4b9749e2e66b2946e27dce7d" },
        M { name: "lex.50.50.nnen.s2t.bin", bytes: 1506236, sha256: "b19abe7154430c1ee321e29391b14b2752c3350c3fbfcd1caf1ef7a0f9a30a97" },
        M { name: "vocab.nnen.spm", bytes: 510942, sha256: "14625622062046b7c44eb79652efb113518bf4b52fa1b98a949934b7a94b0273" },
    ] },
    P { src: "pl", dst: "en", files: [
        M { name: "model.plen.intgemm.alphas.bin", bytes: 17140899, sha256: "172a5f1d44bf8dd6a6eec3868b13b33ce265f3530e898fe11a80b739b821726e" },
        M { name: "lex.50.50.plen.s2t.bin", bytes: 4898024, sha256: "863afade0ba058fb0173fedef3d1fb14d0dcabc24c3b4584cb1fed8f84d6d879" },
        M { name: "vocab.plen.spm", bytes: 822587, sha256: "a1d27e6f5c0d29f406364ebf0382949d1c0affc750cec4380f3173150552f43e" },
    ] },
    P { src: "pt", dst: "en", files: [
        M { name: "model.pten.intgemm.alphas.bin", bytes: 17140899, sha256: "b4a1fd101f59ca258bfb051a3a4e5762497b9f91d7e74f28b7a2d68dc075f8fa" },
        M { name: "lex.50.50.pten.s2t.bin", bytes: 4801740, sha256: "702d9b09fbc020168998bb81afc927e74262f5a1455e9b443608a5d8efa4b52d" },
        M { name: "vocab.pten.spm", bytes: 817234, sha256: "2326a27577b5ffa0b822c55fe850986961728097521de35fcc186f7d6dce72d4" },
    ] },
    P { src: "ru", dst: "en", files: [
        M { name: "model.ruen.intgemm.alphas.bin", bytes: 17140836, sha256: "3b6a0305e3d232fadd54f5a765365b7b96ad6d8f2e818cba594b02fbd8fadb3d" },
        M { name: "lex.50.50.ruen.s2t.bin", bytes: 5090836, sha256: "e6667e22f5f86be4872e3768b7184727f5dd8c9f2ccfb0639baabcb1176f5d11" },
        M { name: "vocab.ruen.spm", bytes: 936576, sha256: "aaf9a325c0a988c507d0312cb6ba1a02bac7a370bcd879aedee626a40bfbda78" },
    ] },
    P { src: "en", dst: "bg", files: [
        M { name: "model.enbg.intgemm.alphas.bin", bytes: 17140899, sha256: "02715a7a81a610a37439d4f788a6f3efcc1ecb39618bc4184442a39378907dfe" },
        M { name: "lex.50.50.enbg.s2t.bin", bytes: 5607608, sha256: "0f9b794b6f8a9c4b5b781fde49391852b398184b730f89a09428cf562e8bede6" },
        M { name: "vocab.bgen.spm", bytes: 919745, sha256: "b14e44beb653db924c826e1696bcfab23ca9fd3e479baf8bea67d0be77432192" },
    ] },
    P { src: "en", dst: "cs", files: [
        M { name: "model.encs.intgemm.alphas.bin", bytes: 17140756, sha256: "9a2fe0588bd972accfc801e2f31c945de0557804a91666ae5ab43b94fb74ac4b" },
        M { name: "lex.50.50.encs.s2t.bin", bytes: 3556124, sha256: "e19c77231bf977988e31ff8db15fe79966b5170564bd3e10613f239e7f461d97" },
        M { name: "vocab.csen.spm", bytes: 769763, sha256: "f71cc5d045e479607078e079884f44032f5a0b82547fb96eefa29cd1eb47c6f3" },
    ] },
    P { src: "en", dst: "de", files: [
        M { name: "model.ende.intgemm.alphas.bin", bytes: 17140835, sha256: "b3e980d6602ab0bdfe8d9315cb5fc282a16bb1c8dccf38e70945c584551c4318" },
        M { name: "lex.50.50.ende.s2t.bin", bytes: 3943644, sha256: "f03eb8245042feb7a5800815b8d0dc215d7a60691632405b65c461d250cedbe6" },
        M { name: "vocab.deen.spm", bytes: 784269, sha256: "417668f2ed297970febafb5b079a9d5ebc4ed0b3550ac8386d67a90473a09bd7" },
    ] },
    P { src: "en", dst: "es", files: [
        M { name: "model.enes.intgemm.alphas.bin", bytes: 17140755, sha256: "fa7460037a3163e03fe1d23602f964bff2331da6ee813637e092ddf37156ef53" },
        M { name: "lex.50.50.enes.s2t.bin", bytes: 3347104, sha256: "3a113d713dec3cf1d12bba5b138ae616e28bba4bbc7fe7fd39ba145e26b86d7f" },
        M { name: "vocab.esen.spm", bytes: 825463, sha256: "909b1eea1face0d7f90a474fe29a8c0fef8d104b6e41e65616f864c964ba8845" },
    ] },
    P { src: "en", dst: "et", files: [
        M { name: "model.enet.intgemm.alphas.bin", bytes: 17140754, sha256: "a28874a8b702a519a14dc71bcee726a5cb4b539eeaada2d06492f751469a1fd6" },
        M { name: "lex.50.50.enet.s2t.bin", bytes: 2700780, sha256: "3d1b40ff43ebef82cf98d416a88a1ea19eb325a85785eef102f59878a63a829d" },
        M { name: "vocab.eten.spm", bytes: 828426, sha256: "e3b66bc141f6123cd40746e2fb9b8ee4f89cbf324ab27d6bbf3782e52f15fa2d" },
    ] },
    P { src: "en", dst: "fa", files: [
        M { name: "model.enfa.intgemm.alphas.bin", bytes: 17140835, sha256: "74fa9694e56c9e0511229c6d247752132b4cf83bbf0574a601dceeac5b62f1b9" },
        M { name: "lex.50.50.enfa.s2t.bin", bytes: 5097656, sha256: "09db90a01fc53764db021c58f4935a60b693b90835039b5a27b7bb9f4709e3b7" },
        M { name: "vocab.enfa.spm", bytes: 794181, sha256: "ecf95b5cf04b86bf68113b3d6fdcf23c2728eb5929cebc0f5aa5c4d8b8330fa4" },
    ] },
    P { src: "en", dst: "fr", files: [
        M { name: "model.enfr.intgemm.alphas.bin", bytes: 17140961, sha256: "0678019c4d74c8c81d2de17e3e58d3aba5f5eb48f5595d9240c17f69d30461de" },
        M { name: "lex.50.50.enfr.s2t.bin", bytes: 7886500, sha256: "38fb44bad1fd5f1e6bfdcf15cc8baa09d61aad2a4f9c587914e24e7b5c25c32c" },
        M { name: "vocab.fren.spm", bytes: 831382, sha256: "4c84b95b62c930f0791466d73eb996841eef474c96d0c2f0e6c6d80640f2005a" },
    ] },
    P { src: "en", dst: "it", files: [
        M { name: "model.enit.intgemm.alphas.bin", bytes: 17140899, sha256: "3d7bbc4d7977e10b35f53faa79f5d5de8211f4f04baed9e7cd9dee1dcceda917" },
        M { name: "lex.50.50.enit.s2t.bin", bytes: 4495004, sha256: "351ea80fb9f366f07533c7c4836248e72d9d4aa4eb7a05b5d74891a7abb4208c" },
        M { name: "vocab.enit.spm", bytes: 812781, sha256: "603f3349657c3deb9736a0c567452d102a5a03c377dfdf1d32c428608f2cff1b" },
    ] },
    P { src: "en", dst: "nl", files: [
        M { name: "model.ennl.intgemm.alphas.bin", bytes: 17140899, sha256: "906690a58a0d72aff28bd4b941cbd0984d1e0a62958c0b21aebae378a656d822" },
        M { name: "lex.50.50.ennl.s2t.bin", bytes: 4494892, sha256: "f780a6d74af4b141f551dcc0da56bab44a05a90ef53d63381269710f35eaa41b" },
        M { name: "vocab.ennl.spm", bytes: 807541, sha256: "43ba3922c3bba2b76ca2e2124837c96518b0e31300b7d6d5ccce55ee10d86393" },
    ] },
    P { src: "en", dst: "pl", files: [
        M { name: "model.enpl.intgemm.alphas.bin", bytes: 17140899, sha256: "60d45f43a5ac869a80f899751d2d1f0f456da9815d26db70e4d2e0fd18ed4a8f" },
        M { name: "lex.50.50.enpl.s2t.bin", bytes: 3642112, sha256: "409fbf5856cec372dffe0a3aa3c89462e2efbd783557272af84800a67195c38c" },
        M { name: "vocab.enpl.spm", bytes: 822587, sha256: "a1d27e6f5c0d29f406364ebf0382949d1c0affc750cec4380f3173150552f43e" },
    ] },
    P { src: "en", dst: "pt", files: [
        M { name: "model.enpt.intgemm.alphas.bin", bytes: 17140899, sha256: "8fb05a27509bea3f67d2f59506485584d5cdbdcafa82b251576c27e91bd7011e" },
        M { name: "lex.50.50.enpt.s2t.bin", bytes: 4345620, sha256: "dd0e64865277f0a1cf15a26a5b50e9e0570235afd8dab7c5e0249ad7057531d6" },
        M { name: "vocab.enpt.spm", bytes: 817234, sha256: "2326a27577b5ffa0b822c55fe850986961728097521de35fcc186f7d6dce72d4" },
    ] },
    P { src: "en", dst: "ru", files: [
        M { name: "model.enru.intgemm.alphas.bin", bytes: 17140836, sha256: "4a45186a93b8a2dd9301c66a3b3dad580b1bcfa74aadda583ca383f9fe0dea93" },
        M { name: "lex.50.50.enru.s2t.bin", bytes: 3049096, sha256: "7bd3e2c0a72286fe1f3da65c56c49a7cd77efa5f1d1a444e2a9e769480b96ff3" },
        M { name: "vocab.enru.spm", bytes: 937157, sha256: "feca2d44f01b946c85faba3b15b5eb53344bec84cd14a1a4d4a82ddd774c5edd" },
    ] },
];

/// The pair for `src` read as `dst`, if Lumen carries it.
pub fn pair(src: &str, dst: &str) -> Option<&'static P> {
    PAIRS.iter().find(|p| p.src == src && p.dst == dst)
}

/// The two-letter code in a lang attribute: "es-419", "EN_us" and "en" are
/// "es", "en", "en". Empty when there is nothing to read.
pub fn lang_code(raw: &str) -> String {
    raw.trim()
        .to_ascii_lowercase()
        .split(['-', '_'])
        .next()
        .unwrap_or("")
        .chars()
        .take(2)
        .collect()
}

/// What language the page is in. A page that does not say gets the person's own
/// Windows language: there is no text detector here, and that is the honest guess.
pub fn page_lang(html_lang: &str) -> String {
    let said = lang_code(html_lang);
    if said.chars().count() == 2 {
        said
    } else {
        lang_code(&system_lang())
    }
}

/// The Windows user interface language, from kernel32. Empty when it cannot be had.
fn system_lang() -> String {
    use windows::core::{PCSTR, w};
    use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
    unsafe {
        let Ok(k32) = GetModuleHandleW(w!("kernel32.dll")) else { return String::new() };
        let Some(f) = GetProcAddress(k32, PCSTR(c"GetUserDefaultLocaleName".as_ptr() as *const u8)) else { return String::new() };
        let f: extern "system" fn(*mut u16, i32) -> i32 = std::mem::transmute(f);
        let mut buf = [0u16; 85];
        let n = f(buf.as_mut_ptr(), buf.len() as i32);
        if n <= 0 {
            return String::new();
        }
        String::from_utf16_lossy(&buf[..(n as usize).saturating_sub(1)])
    }
}

/// The folder the translator page is served from: data_dir()/bergamot.
pub fn folder() -> PathBuf {
    crate::store::data_dir().join("bergamot")
}

/// Where a pair's model files live: data_dir()/models/<pair>.
pub fn model_dir(p: &P) -> PathBuf {
    crate::store::data_dir().join("models").join(p.name())
}

/// Put the engine and Lumen's own scripts under data_dir()/bergamot/. The two
/// bundled files are checked against their pins and rewritten when they differ;
/// Lumen's own three change with the build and are always written. True when the
/// folder is ready to serve.
pub fn install() -> bool {
    let dir = folder();
    if fs::create_dir_all(&dir).is_err() {
        return false;
    }
    if !put(&dir.join("bergamot-translator-worker.js"), GLUE.as_bytes(), GLUE_SHA256) {
        return false;
    }
    if !put(&dir.join("bergamot-translator-worker.wasm"), WASM, WASM_SHA256) {
        return false;
    }
    fs::write(dir.join("lumen-worker.js"), WORKER).is_ok()
        && fs::write(dir.join("lumen-translate.js"), DRIVER).is_ok()
        && fs::write(dir.join("index.html"), PAGE).is_ok()
}

/// Write `bytes` unless the file already holds exactly them. tmp + rename, so a
/// crash never leaves half an engine behind.
fn put(path: &Path, bytes: &[u8], sha: &str) -> bool {
    if fs::read(path).is_ok_and(|old| hex(&old) == sha) {
        return true;
    }
    let tmp = path.with_extension("part");
    if fs::write(&tmp, bytes).is_err() {
        return false;
    }
    fs::rename(&tmp, path).is_ok()
}

fn hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// Every file of this pair is on disk and hashes right. The UI does not ask: the
/// fetch step already skips what is there, and hashing 20 MB on the UI thread to
/// find that out is the one thing it must not do.
#[allow(dead_code)]
pub fn ready(p: &P) -> bool {
    let dir = model_dir(p);
    p.files.iter().all(|f| fs::read(dir.join(f.name)).is_ok_and(|b| hex(&b) == f.sha256))
}

/// Fetch whatever of the pair is missing or wrong, checking each file against the
/// registry before it is kept. Blocks: run it on its own thread. The first use of a
/// pair costs 15-25 MB of download.
pub fn fetch(p: &'static P) -> Result<(), String> {
    let dir = model_dir(p);
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    for f in &p.files {
        let path = dir.join(f.name);
        if fs::read(&path).is_ok_and(|b| hex(&b) == f.sha256) {
            continue;
        }
        let url = format!("{MODELS_ROOT}/{}/{name}", p.name(), name = f.name);
        fetch_verified(&url, &path, f).inspect_err(|_| {
            // A file that did not check out is not left where it could be used.
            let _ = fs::remove_file(&path);
        })?;
    }
    Ok(())
}

/// One model file: streamed to a .part beside it, hashed on the way, kept only when
/// its size and SHA-256 are the registry's. Returns the bytes read.
fn fetch_verified(url: &str, dest: &Path, f: &M) -> Result<u64, String> {
    let part = dest.with_extension("part");
    let _ = fs::remove_file(&part);
    let got = stream_to(url, &part)?;
    let right = got == f.bytes && fs::read(&part).is_ok_and(|b| hex(&b) == f.sha256);
    if !right {
        let _ = fs::remove_file(&part);
        return Err("not the file the registry lists, refused".into());
    }
    fs::rename(&part, dest).map_err(|e| e.to_string())?;
    Ok(got)
}

fn stream_to(url: &str, to: &Path) -> Result<u64, String> {
    let config = ureq::config::Config::builder().timeout_global(Some(FETCH_TIMEOUT)).build();
    let agent = ureq::Agent::new_with_config(config);
    let reply = agent.get(url).call().map_err(|e| e.to_string())?;
    let mut body = reply.into_body().into_with_config().limit(MAX_FETCH).reader();
    let mut f = fs::File::create(to).map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; 256 * 1024];
    let mut total = 0u64;
    loop {
        let n = body.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        std::io::Write::write_all(&mut f, &buf[..n]).map_err(|e| e.to_string())?;
        total += n as u64;
    }
    Ok(total)
}

/// The hidden page that runs the wasm for one translation. Dropping it takes the
/// worker, its model memory and a whole renderer process with it, which is what
/// "tear the worker down after" means here.
pub struct Page {
    view: WebView,
}

impl Page {
    /// Open the translator page for a pair. It reads the engine and the model files
    /// off the disk, so the caller waits for it to say "ready" (`Ev::TrMsg`, once the
    /// model is up) before handing it text. None when there is no page of Lumen's own
    /// to build it from, or the folder cannot be served.
    pub fn open(ctx: &Rc<Ctx>, p: &'static P) -> Option<Page> {
        if !install() {
            eprintln!("lumen: translation: could not write {}", folder().display());
            return None;
        }
        let bounds = Rect {
            position: LogicalPosition::new(0.0, 0.0).into(),
            size: LogicalSize::new(480.0, 360.0).into(),
        };
        let view = crate::engine::build_hidden(ctx, bounds)?;
        // The page reads the engine and the models off the disk through a host name
        // mapped to data_dir(). DENY: only pages of that host may look. The mapping
        // has to be in place before the navigation, or the page never arrives.
        let mapped = unsafe {
            view.webview()
                .cast::<ICoreWebView2_3>()
                .and_then(|c3| {
                    c3.SetVirtualHostNameToFolderMapping(
                        &HSTRING::from(HOST),
                        &HSTRING::from(crate::store::data_dir().to_string_lossy().as_ref()),
                        COREWEBVIEW2_HOST_RESOURCE_ACCESS_KIND_DENY,
                    )
                })
                .is_ok()
        };
        if !mapped {
            eprintln!("lumen: translation: the data folder could not be served");
            return None;
        }
        let proxy = ctx.proxy.clone();
        let mut token = 0i64;
        unsafe {
            let _ = view.webview().add_WebMessageReceived(
                &WebMessageReceivedEventHandler::create(Box::new(move |_, args| {
                    let Some(args) = args else { return Ok(()) };
                    let mut m = PWSTR::null();
                    if args.WebMessageAsJson(&mut m).is_err() {
                        return Ok(());
                    }
                    let _ = proxy.send_event(Ev::TrMsg(crate::engine::take(m)));
                    Ok(())
                })),
                &mut token,
            );
        }
        // The pair and its file names travel in the query string: the page has
        // everything it needs to fetch the model before it says a word.
        let name = p.name();
        let url = format!(
            "https://{HOST}/bergamot/index.html?dir=%2Fmodels%2F{name}%2F&model={}&lex={}&vocab={}&precision={}",
            p.files[0].name,
            p.files[1].name,
            p.files[2].name,
            // The registry's own rule: the "intgemm8" models want the other kernel.
            if p.files[0].name.ends_with("intgemm8.bin") { "int8shiftAll" } else { "int8shiftAlphaAll" },
        );
        if view.load_url(&url).is_err() {
            eprintln!("lumen: translation: the translator page would not load");
            return None;
        }
        Some(Page { view })
    }

    /// Hand the page a JSON array of texts. It answers with `Ev::TrMsg` progress
    /// and then the translations, in the same order.
    pub fn run(&self, texts_json: &str) {
        let _ = self.view.evaluate_script(&format!("window.lumenTranslate({texts_json})"));
    }
}

/// What a page does with F8: the text it will translate, and the way back.
pub const COLLECT: &str = COLLECT_JS;
/// Put the translations in, in the order they were collected.
pub fn apply(texts_json: &str) -> String {
    format!("window.__lumenTrApply({texts_json})")
}
/// Back to the page as it was.
pub const RESTORE: &str = "try{window.__lumenTrRestore&&window.__lumenTrRestore()}catch(e){}";

/// Runs in the page being read. Collects the text nodes worth translating and
/// leaves behind what it takes to put them back: the originals, and the whitespace
/// around each one, which is what keeps inline markup from running together.
/// Scripts, code, form controls and anything marked translate="no" are left alone.
const COLLECT_JS: &str = r##"(()=>{try{
if(window.__lumenTr&&window.__lumenTr.nodes&&window.__lumenTr.nodes.length)
  return {lang:window.__lumenTr.lang,texts:window.__lumenTr.texts,again:1};
const SKIP=new Set(['SCRIPT','STYLE','NOSCRIPT','TEXTAREA','CODE','PRE','KBD','SAMP','VAR','SVG','MATH','IFRAME','CANVAS','VIDEO','AUDIO','SELECT','OPTION']);
const nodes=[],texts=[],lead=[],trail=[];
const want=t=>typeof t==='string'&&/\p{L}/u.test(t)&&t.trim().length>1;
const walk=document.createTreeWalker(document.body||document.documentElement,NodeFilter.SHOW_TEXT,{acceptNode(n){
  const t=n.nodeValue;
  if(!want(t))return NodeFilter.FILTER_REJECT;
  const p=n.parentElement;
  if(!p||SKIP.has(p.tagName))return NodeFilter.FILTER_REJECT;
  if(p.closest('[translate="no"]'))return NodeFilter.FILTER_REJECT;
  if(p.isContentEditable)return NodeFilter.FILTER_REJECT;
  return NodeFilter.FILTER_ACCEPT;
}});
let n;
while((n=walk.nextNode())){
  if(nodes.length>=1500)break;
  const raw=n.nodeValue,t=raw.trim(),i=raw.indexOf(t);
  nodes.push(n);texts.push(t);lead.push(raw.slice(0,i));trail.push(raw.slice(i+t.length));
}
const lang=(''+(document.documentElement.getAttribute('lang')||document.documentElement.lang||'')).toLowerCase();
window.__lumenTr={nodes:nodes,orig:nodes.map(x=>x.nodeValue),texts:texts,lead:lead,trail:trail,lang:lang};
window.__lumenTrApply=arr=>{
  const S=window.__lumenTr;
  if(!S||!S.nodes)return 0;
  let put=0;
  for(let i=0;i<S.nodes.length;i++){
    const v=arr&&arr[i];
    // Nothing came back for this node: the original text stays where it was.
    S.nodes[i].nodeValue=(typeof v==='string'&&v)?S.lead[i]+v+S.trail[i]:S.orig[i];
    put++;
  }
  return put;
};
window.__lumenTrRestore=()=>{
  const S=window.__lumenTr;
  if(!S||!S.nodes)return 0;
  let back=0;
  for(let i=0;i<S.nodes.length;i++){S.nodes[i].nodeValue=S.orig[i];back++;}
  window.__lumenTr=null;
  return back;
};
return {lang:lang,texts:texts};
}catch(e){return {lang:'',texts:[],err:String(e)}}})()"##;

/// The worker behind the translator page: Mozilla's glue, the bundled wasm and the
/// model files, driven the way Firefox Translations itself drives them. Batches
/// keep one huge call from taking minutes without a word to the page.
const WORKER: &str = r##"// Lumen's Bergamot worker. The glue comes in with importScripts, the wasm and the
// model files are fetched over Lumen's own host name, and the model is handed to
// the engine as aligned wasm memory.
importScripts('bergamot-translator-worker.js');

const ALIGN={model:256,lex:64,vocab:64};
const BATCH=48,BATCH_CHARS=4000;
let M=null,service=null,model=null;

const say=m=>postMessage(m);

async function bytes(url){
  const r=await fetch(url);
  if(!r.ok)throw new Error(url+': '+r.status);
  return await r.arrayBuffer();
}
function aligned(buf,alignment){
  const mem=new M.AlignedMemory(buf.byteLength,alignment);
  mem.getByteArrayView().set(new Int8Array(buf));
  return mem;
}

async function load(cfg){
  const wasm=await bytes('bergamot-translator-worker.wasm');
  let up=null;const started=new Promise(r=>{up=r});
  const glue=loadEmscriptenGlueCode({wasmBinary:wasm,onRuntimeInitialized:()=>up()});
  // Older builds hand back the module before its runtime is up; newer ones a promise.
  const mod=await Promise.resolve(glue);
  if(typeof mod.TranslationModel!=='function')await started;
  M=mod;
  if(typeof M.TranslationModel!=='function')throw new Error('engine came up without its model classes');
  const [m,l,v]=await Promise.all([bytes(cfg.dir+cfg.model),bytes(cfg.dir+cfg.lex),bytes(cfg.dir+cfg.vocab)]);
  const vocab=new M.AlignedMemoryList();
  vocab.push_back(aligned(v,ALIGN.vocab));
  // Marian's own options. The spaces between the entries matter to the parser.
  const config=`
            beam-size: 1
            normalize: 1.0
            word-penalty: 0
            max-length-break: 128
            mini-batch-words: 1024
            workspace: 128
            max-length-factor: 2.0
            skip-cost: true
            cpu-threads: 0
            quiet: true
            quiet-translation: true
            gemm-precision: ${cfg.precision}
            alignment: soft
            `;
  model=new M.TranslationModel(config,aligned(m,ALIGN.model),aligned(l,ALIGN.lex),vocab,null);
  service=new M.BlockingService({cacheSize:0});
  say({t:'loaded'});
}

function run(texts){
  const src=new M.VectorString();
  for(const s of texts){const t=s.trim();if(t)src.push_back(t)}
  const opts=new M.VectorResponseOptions();
  for(let i=0;i<src.size();i++)opts.push_back({qualityScores:false,alignment:true,html:false});
  const res=service.translate(model,src,opts);
  const out=[];
  for(let i=0;i<res.size();i++)out.push(res.get(i).getTranslatedText());
  res.delete();opts.delete();src.delete();
  return out;
}

onmessage=async e=>{
  const d=e.data||{};
  try{
    if(d.t==='load'){await load(d.cfg);return}
    if(d.t!=='go')return;
    const texts=d.texts||[],out=new Array(texts.length).fill('');
    let at=0;
    while(at<texts.length){
      let end=at,chars=0;
      while(end<texts.length&&end-at<BATCH&&chars<BATCH_CHARS){chars+=texts[end].length+1;end++}
      const got=run(texts.slice(at,end));
      for(let i=0;i<got.length&&at+i<out.length;i++)out[at+i]=got[i];
      at=end;
      say({t:'progress',done:at,total:texts.length});
      // Let the progress message out before the next batch takes the thread.
      await new Promise(r=>setTimeout(r,0));
    }
    say({t:'texts',texts:out});
  }catch(err){say({t:'err',err:(err&&err.message)||String(err)})}
};
"##;

/// The page itself. Nothing but the driver; there is no markup to show.
const PAGE: &str = r##"<!doctype html><html lang="en"><head><meta charset="utf-8">
<title>Lumen translate</title></head><body><script src="lumen-translate.js"></script></body></html>
"##;

/// The translator page's own script: the host's one call, and the worker's
/// messages on the way back to it. Where the model files are came in the query
/// string, so nothing has to be said before the engine is up.
const DRIVER: &str = r##"// Lumen's translator page. The host hands it text with window.lumenTranslate(texts)
// once it says it is ready; nothing here talks to the network on its own.
const W=window.chrome&&window.chrome.webview;
const q=new URLSearchParams(location.search);
const cfg={
  dir:q.get('dir')||'',model:q.get('model')||'',lex:q.get('lex')||'',vocab:q.get('vocab')||'',
  precision:q.get('precision')||'int8shiftAlphaAll',
};
let worker=null,loaded=false,queued=null;
const tell=m=>{if(W)W.postMessage(m)};

function run(){
  if(!loaded||!queued)return;
  const texts=queued;
  queued=null;
  worker.postMessage({t:'go',texts:texts});
}
function boot(){
  worker=new Worker('lumen-worker.js');
  worker.onmessage=e=>{
    const d=e.data||{};
    if(d.t==='loaded'){loaded=true;tell({lumen:'tr',ready:1});run()}
    else if(d.t==='progress')tell({lumen:'tr',p:d.done,n:d.total});
    else if(d.t==='texts')tell({lumen:'tr',texts:d.texts});
    else if(d.t==='err')tell({lumen:'tr',err:String(d.err)});
  };
  worker.onerror=e=>tell({lumen:'tr',err:'worker: '+(e.message||'failed to start')});
  worker.postMessage({t:'load',cfg:cfg});
}
window.lumenTranslate=texts=>{queued=texts;run()};
boot();
"##;

#[cfg(test)]
mod tests {
    use super::*;

    /// The bundled engine is the one the pins describe. A swapped file fails here.
    #[test]
    fn bundled_engine_matches_its_pins() {
        assert_eq!(hex(GLUE.as_bytes()), GLUE_SHA256);
        assert_eq!(hex(WASM), WASM_SHA256);
        // Whatever the registry says a file is, it is a file we can name.
        assert!(WASM.len() as u64 > 1_000_000);
    }

    #[test]
    fn registry_entries_are_well_formed() {
        assert!(PAIRS.len() > 20, "the registry has many pairs");
        for p in PAIRS {
            assert_eq!(p.src.len(), 2, "{}", p.name());
            assert_eq!(p.dst.len(), 2, "{}", p.name());
            assert!(p.src != p.dst);
            assert_eq!(p.files.len(), 3);
            for f in &p.files {
                assert_eq!(f.sha256.len(), 64, "{}", f.name);
                assert!(f.sha256.chars().all(|c| c.is_ascii_hexdigit()));
                assert!(f.bytes > 100_000, "{} is not a model file", f.name);
            }
            // Model, shortlist, vocabulary: the worker's load config is in this order.
            assert!(p.files[0].name.starts_with("model.") && p.files[0].name.ends_with(".bin"), "{}", p.files[0].name);
            assert!(p.files[1].name.starts_with("lex."), "{}", p.files[1].name);
            assert!(p.files[2].name.starts_with("vocab."), "{}", p.files[2].name);
            // No pair twice.
            assert!(PAIRS.iter().filter(|q| q.name() == p.name()).count() == 1);
        }
        // The pair the tests translate with is really there.
        let p = pair("es", "en").expect("spanish to english");
        assert_eq!(p.files[0].name, "model.esen.intgemm.alphas.bin");
    }

    #[test]
    fn language_codes_come_down_to_two_letters() {
        assert_eq!(lang_code("es-419"), "es");
        assert_eq!(lang_code("EN_us"), "en");
        assert_eq!(lang_code(" fr "), "fr");
        assert_eq!(lang_code("nonsense"), "no");
        assert_eq!(lang_code(""), "");
        // A page that says nothing gets the system language, whatever that is.
        let guess = page_lang("");
        assert!(guess.is_empty() || guess.chars().count() == 2);
        assert_eq!(page_lang("de-DE"), "de");
    }

    #[test]
    fn the_translator_scripts_carry_their_entry_points() {
        assert!(COLLECT.contains("__lumenTrApply") && COLLECT.contains("__lumenTrRestore"));
        assert!(apply("[\"a\",\"b\"]").starts_with("window.__lumenTrApply(["));
        assert!(RESTORE.contains("__lumenTrRestore"));
        assert!(WORKER.contains("loadEmscriptenGlueCode") && WORKER.contains("TranslationModel"));
        assert!(DRIVER.contains("window.lumenTranslate") && DRIVER.contains("new Worker"));
        assert!(PAGE.contains("lumen-translate.js"));
    }

    /// A pair's files go under data_dir()/models/<pair>/, where the page finds them.
    #[test]
    fn model_files_live_under_the_pair_folder() {
        let p = pair("fr", "en").expect("french to english");
        assert!(model_dir(p).ends_with("models/fren") || model_dir(p).ends_with("models\\fren"));
        assert!(!ready(p), "nothing is downloaded in a test run");
    }
}
