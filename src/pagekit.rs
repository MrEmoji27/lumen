//! Things Lumen does to a page: dark mode for sites that have none (Dark Reader, MIT)
//! and reader mode (Mozilla Readability, Apache-2.0, sanitized by DOMPurify).
//!
//! Bundled, pinned (SHA-256 of the files in assets/):
//!   darkreader 4.9.133      82619e7a0bcabbea15a91488bc74fd0f9ba8f8f2a1cc67527f78166037cce166
//!   readability 0.6.0       34dcab3d0832d0019f02990eed6b6124e029e8c32b9f0c6f2550544ff8dff174
//!   readerable  0.6.0       a98d28805804c1986ceed470678a3f409f150ee7f1d227f8c8239c005d21de65
//!   dompurify   3.4.17      cd37898f9782adf54820c413191b5691803f0f137c799d6edcfd38523b4c2c31

use base64::Engine as _;
use serde_json::Value;

const DARKREADER: &str = include_str!("../assets/darkreader/darkreader.js");
const READABILITY: &str = include_str!("../assets/readability/Readability.js");
const READERABLE: &str = include_str!("../assets/readability/Readability-readerable.js");
const PURIFY: &str = include_str!("../assets/readability/purify.min.js");
const SERIF: &[u8] = include_bytes!("../ui/fonts/serif.woff2");

/// Colors Dark Reader paints with, from Lumen's palette.
pub struct Look {
    pub bg: String,
    pub text: String,
}

/// Runs at document creation, top frame only. Defines Dark Reader once, then switches it on
/// after the DOM exists, unless the page is already dark (many sites follow the dark system
/// theme) or its host is in `skip`. Run again with new colors, it re-enables with those.
pub fn dark_script(look: &Look, skip: &[String]) -> String {
    let cfg = serde_json::json!({
        "brightness": 100, "contrast": 95, "sepia": 0,
        "darkSchemeBackgroundColor": look.bg, "darkSchemeTextColor": look.text,
    });
    let skip = serde_json::to_string(skip).unwrap_or_else(|_| "[]".into());
    format!(
        r#"(()=>{{if(window!==window.top)return;
if({skip}.includes(location.hostname)){{try{{window.DarkReader&&window.DarkReader.disable()}}catch(e){{}}return;}}
if(!window.DarkReader){{(function(exports,module,define){{
{DARKREADER}
}}).call(window);}}
const DR=window.DarkReader;
const go=()=>{{try{{
 if(DR&&DR.isEnabled()){{DR.enable({cfg});return;}}
 const el=document.body||document.documentElement;
 const bg=getComputedStyle(el).backgroundColor||'';
 const n=(bg.match(/[\d.]+/g)||[]).map(Number);
 const clear=n.length<3||(n.length>3&&n[3]===0);
 const lum=clear?1:(0.2126*n[0]+0.7152*n[1]+0.0722*n[2])/255;
 if(!clear&&lum<0.4)return;
 if(DR){{DR.setFetchMethod(window.fetch);DR.enable({cfg});}}
}}catch(e){{}}}};
if(document.readyState==='loading')document.addEventListener('DOMContentLoaded',go,{{once:true}});else go();}})();"#
    )
}

/// Turns Dark Reader off in an open page.
pub const DARK_OFF: &str = "try{window.DarkReader&&window.DarkReader.disable()}catch(e){}";

/// Runs in the page; returns the article as JSON, or null when there is none.
pub fn extract_script() -> String {
    format!(
        r#"(()=>{{try{{
const lib=(function(exports,module,define){{
{PURIFY}
{READABILITY}
{READERABLE}
return {{Readability,isProbablyReaderable,DOMPurify:window.DOMPurify}};
}}).call(window);
const {{Readability,DOMPurify}}=lib;
const doc=document.cloneNode(true);
const a=new Readability(doc,{{charThreshold:300}}).parse();
if(!a||!a.content)return null;
const clean=DOMPurify.sanitize(a.content,{{FORBID_TAGS:['style','form','input','button','iframe'],FORBID_ATTR:['style']}});
return {{title:a.title||document.title,byline:a.byline||'',siteName:a.siteName||'',content:clean,length:a.length||0,lang:a.lang||document.documentElement.lang||''}};
}}catch(e){{return null}}}})()"#
    )
}

/// Reader page colors and type.
pub struct Style {
    pub dark: bool,
    pub bg: String,
    pub text: String,
    pub dim: String,
    pub accent: String,
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// A standalone page: no scripts (strict CSP), images kept, Lumen typography.
pub fn render(article: &Value, original: &str, st: &Style) -> String {
    let get = |k: &str| article.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
    let (title, byline, site) = (esc(&get("title")), esc(&get("byline")), esc(&get("siteName")));
    // DOMPurify already sanitized it; the CSP below is the second lock.
    let content = get("content").replace("<script", "&lt;script");
    // About 230 words a minute, about 5.2 characters a word.
    let chars = article.get("length").and_then(|v| v.as_u64()).unwrap_or(0) as f64;
    let minutes = ((chars / 1200.0).round() as u64).max(1);
    let font = base64::engine::general_purpose::STANDARD.encode(SERIF);
    let meta = [site.as_str(), byline.as_str()].iter().filter(|s| !s.is_empty()).cloned().collect::<Vec<_>>().join(" · ");
    format!(
        r#"<!doctype html><html lang="{lang}"><head><meta charset="utf-8">
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; img-src * data:; media-src *; style-src 'unsafe-inline'; font-src data:">
<title>{title}</title><style>
@font-face{{font-family:'Instrument Serif';src:url(data:font/woff2;base64,{font}) format('woff2')}}
:root{{color-scheme:{scheme}}}
html{{background:{bg};color:{text}}}
body{{margin:0;padding:64px 24px 120px;font:19px/1.7 Charter,'Iowan Old Style','Georgia',serif;-webkit-font-smoothing:antialiased}}
main{{max-width:680px;margin:0 auto}}
.src{{font:13px/1.4 'Segoe UI Variable Text',system-ui,sans-serif;color:{dim};margin-bottom:28px;letter-spacing:.01em}}
.src a{{color:{dim}}}
h1{{font:400 46px/1.08 'Instrument Serif',Georgia,serif;letter-spacing:-.01em;margin:0 0 14px}}
h2,h3,h4{{font-family:'Instrument Serif',Georgia,serif;font-weight:400;line-height:1.2;margin:1.6em 0 .5em}}
h2{{font-size:31px}}h3{{font-size:25px}}
p{{margin:0 0 1.1em}}
a{{color:{accent};text-decoration-thickness:1px;text-underline-offset:3px}}
img,video,figure{{max-width:100%;height:auto;border-radius:8px}}
figure{{margin:1.6em 0}}figcaption{{font-size:14px;color:{dim};margin-top:6px}}
blockquote{{margin:1.4em 0;padding-left:18px;border-left:2px solid {accent};color:{dim}}}
pre,code{{font:15px/1.5 'Cascadia Code',Consolas,monospace}}pre{{overflow:auto;padding:14px;border-radius:8px;background:color-mix(in srgb,{text} 7%,{bg})}}
hr{{border:0;height:1px;background:color-mix(in srgb,{text} 15%,{bg});margin:2em 0}}
table{{border-collapse:collapse;font-size:16px}}td,th{{padding:6px 10px;border-bottom:1px solid color-mix(in srgb,{text} 12%,{bg})}}
</style></head><body><main>
<div class="src"><a href="{url}">{host}</a>{meta_sep}{meta} · {minutes} min read</div>
<h1>{title}</h1>
{content}
</main></body></html>"#,
        lang = esc(&get("lang")),
        scheme = if st.dark { "dark" } else { "light" },
        bg = st.bg,
        text = st.text,
        dim = st.dim,
        accent = st.accent,
        url = esc(original),
        host = esc(original.split("://").nth(1).and_then(|r| r.split('/').next()).unwrap_or(original)),
        meta_sep = if meta.is_empty() { "" } else { " · " },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn style() -> Style {
        Style { dark: true, bg: "#111".into(), text: "#eee".into(), dim: "#999".into(), accent: "#fa0".into() }
    }

    #[test]
    fn render_escapes_and_locks() {
        let a = serde_json::json!({"title": "A <b>bold</b> \"title\"", "byline": "x", "content": "<p>hi</p><script>bad()</script>", "length": 3000});
        let html = render(&a, "https://ex.com/post", &style());
        assert!(html.contains("A &lt;b&gt;bold&lt;/b&gt; &quot;title&quot;"));
        assert!(!html.contains("<script>"));
        assert!(html.contains("Content-Security-Policy"));
        assert!(html.contains("3 min read"));
        assert!(html.contains("ex.com"));
    }

    #[test]
    fn scripts_embed_libraries() {
        assert!(extract_script().contains("Readability"));
        assert!(extract_script().contains("DOMPurify"));
        let d = dark_script(&Look { bg: "#181a1b".into(), text: "#e8e6e3".into() }, &["ex.com".into()]);
        assert!(d.contains("DarkReader") && d.contains("#181a1b") && d.contains("[\"ex.com\"]"));
    }
}
