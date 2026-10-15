//! Read aloud. Windows' own speech engine (SAPI, SpVoice), one sentence at a
//! time, so the reader page can highlight the sentence being spoken. The audio
//! never leaves the machine: there is no network anywhere in this file.

use std::sync::OnceLock;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::time::Duration;

use windows::Win32::Media::Speech::{ISpVoice, SPRS_DONE, SPVOICESTATUS, SVSFlagsAsync, SVSFPurgeBeforeSpeak, SVSFIsNotXML};
use windows::Win32::System::Com::{CLSCTX_ALL, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx};
use windows::core::{GUID, PCWSTR};

/// How often the voice is asked whether the sentence finished.
const POLL: Duration = Duration::from_millis(50);

/// The system's speech object, by CLSID. The windows crate's Speech bindings
/// do not carry the class id, so it is named here (the well-known SpVoice id).
const CLSID_SPVOICE: GUID = GUID::from_u128(0x96749B77_8922_48FD_9F48_41FB524B4C11);

enum Cmd {
    Play {
        sentences: Vec<String>,
        line: Box<dyn Fn(usize) + Send>,
        end: Box<dyn Fn() + Send>,
    },
    Pause,
    Resume,
    Stop,
}

struct Job {
    sentences: Vec<String>,
    line: Box<dyn Fn(usize) + Send>,
    end: Box<dyn Fn() + Send>,
}

/// The speaker thread, born on the first call. None means: no voice on this
/// machine (no SAPI, or the build of Windows has none) — every call becomes a
/// no-op and the chip in the sidebar just does nothing when pressed.
fn tx() -> Option<&'static Sender<Cmd>> {
    static TX: OnceLock<Option<Sender<Cmd>>> = OnceLock::new();
    TX.get_or_init(|| {
        let (tx, rx) = channel::<Cmd>();
        std::thread::Builder::new()
            .name("lumen-aloud".into())
            .spawn(move || voice_loop(rx))
            .ok()
            .map(|_| tx)
    })
    .as_ref()
}

/// Start reading. `line(n)` fires as sentence `n` starts; `end` fires once the
/// last one is done (not after Stop — the caller already knows).
pub fn speak(sentences: Vec<String>, line: impl Fn(usize) + Send + 'static, end: impl Fn() + Send + 'static) -> bool {
    let Some(t) = tx() else { return false };
    t.send(Cmd::Play {
        sentences,
        line: Box::new(line),
        end: Box::new(end),
    })
    .is_ok()
}

pub fn pause() {
    if let Some(t) = tx() {
        let _ = t.send(Cmd::Pause);
    }
}

pub fn resume() {
    if let Some(t) = tx() {
        let _ = t.send(Cmd::Resume);
    }
}

pub fn stop() {
    if let Some(t) = tx() {
        let _ = t.send(Cmd::Stop);
    }
}

fn voice_loop(rx: Receiver<Cmd>) {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }
    let voice = unsafe { CoCreateInstance::<_, ISpVoice>(&CLSID_SPVOICE, None, CLSCTX_ALL) }.ok();
    let mut job: Option<Job> = None;
    let mut at = 0usize;
    let mut paused = false;
    // The sentence in the voice has been queued; its status is worth polling.
    let mut said = false;
    loop {
        // A command answers at once; between them, poll the voice.
        // recv() reports a closed channel as RecvError; same meaning here.
        let cmd = if job.is_some() {
            rx.recv_timeout(POLL)
        } else {
            rx.recv().map_err(|_| RecvTimeoutError::Disconnected)
        };
        match cmd {
            Ok(Cmd::Play { sentences, line, end }) => {
                if voice.is_some() && !sentences.is_empty() {
                    at = 0;
                    paused = false;
                    said = false;
                    job = Some(Job {
                        sentences,
                        line,
                        end,
                    });
                }
            }
            Ok(Cmd::Pause) => {
                if let (Some(v), true) = (&voice, job.is_some() && !paused) {
                    unsafe {
                        let _ = v.Pause();
                    }
                    paused = true;
                }
            }
            Ok(Cmd::Resume) => {
                if let (Some(v), true) = (&voice, job.is_some() && paused) {
                    unsafe {
                        let _ = v.Resume();
                    }
                    paused = false;
                }
            }
            Ok(Cmd::Stop) => {
                // ISpVoice has no Stop: purging the queue stops the voice.
                if let Some(v) = &voice {
                    say(v, "");
                }
                job = None;
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        let mut spoken_through = false;
        match (&voice, &mut job) {
            (Some(v), Some(j)) => {
                if !said {
                    say(v, &j.sentences[at]);
                    (j.line)(at);
                    said = true;
                } else if !paused && finished(v) {
                    at += 1;
                    if at >= j.sentences.len() {
                        spoken_through = true;
                    } else {
                        said = false;
                    }
                }
            }
            _ => continue,
        }
        if spoken_through {
            if let Some(j) = job.take() {
                (j.end)();
            }
        }
    }
}

/// Queue one sentence. Async, so the loop keeps answering commands while it
/// speaks.
fn say(v: &ISpVoice, text: &str) {
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    // Purge whatever was queued, speak as plain text (a sentence may contain
    // a stray "<" that SAPI would otherwise read as XML markup).
    const FLAGS: u32 = (SVSFlagsAsync.0 | SVSFPurgeBeforeSpeak.0 | SVSFIsNotXML.0) as u32;
    unsafe {
        let _ = v.Speak(PCWSTR(wide.as_ptr()), FLAGS, None);
    }
    // SAPI queues the text, then starts the audio. Reading the status right
    // away would say "done" before a sound came out.
    std::thread::sleep(Duration::from_millis(30));
}

fn finished(v: &ISpVoice) -> bool {
    let mut st = SPVOICESTATUS::default();
    unsafe { v.GetStatus(&mut st, std::ptr::null_mut()).is_ok() && st.dwRunningState == SPRS_DONE.0 as u32 }
}

// ------------------------------------------------------------- page scripts

/// Split the reader page's visible text into sentences, in the page itself.
/// Returns [[start, end, text], …] over one flat character index across all
/// text nodes, so Rust can speak the text and the page can find it again for
/// the highlight.
pub fn prep_script() -> String {
    r#"(()=>{
  const w=document.createTreeWalker(document.body,NodeFilter.SHOW_TEXT,{
    acceptNode(n=>{
      const p=n.parentElement;
      if(!p||!n.nodeValue.trim()) return NodeFilter.FILTER_REJECT;
      if(p.closest('script,style,nav,header,footer,aside,button')) return NodeFilter.FILTER_REJECT;
      const r=p.getBoundingClientRect();
      if(r.width<1||r.height<1) return NodeFilter.FILTER_REJECT;
      return NodeFilter.FILTER_ACCEPT;
    }});
  let text=''; const nodes=[];
  for(let n=w.nextNode();n;n=w.nextNode()){ nodes.push([n,text.length]); text+=n.nodeValue; }
  window.__lumenAloud={text,nodes,ranges:[]};
  if(!text.trim()) return [];
  const out=[];
  const add=(e)=>{
    let a=s,b=e;
    while(a<b&&/\s/.test(text[a]))a++;
    while(b>a&&/\s/.test(text[b-1]))b--;
    if(b-a>=2){ window.__lumenAloud.ranges.push([a,b]); out.push([a,b,text.slice(a,b)]); }
  };
  let s=0;
  for(let i=0;i<text.length;i++){
    const c=text[i];
    if(c==='\n'){ add(i); s=i+1; continue; }
    if(c==='.'||c==='!'||c==='?'){
      let j=i+1;
      while(j<text.length&&/[)\u201d\u2019'"]/.test(text[j]))j++;
      if(j>=text.length||/\s/.test(text[j])){ add(j); s=j; }
    }
  }
  add(text.length);
  return out;
})()"#
        .to_string()
}

/// Highlight one sentence (or none, to wipe it) with the CSS Custom Highlight
/// API. Degrades to nothing where the API is missing.
pub fn highlight_script(n: Option<usize>) -> String {
    let n = n.map(|k| k as i64).unwrap_or(-1);
    format!(
        r#"(()=>{{
  const n={n}; const key='lumen-aloud';
  if(typeof CSS==='undefined'||!CSS.highlights) return false;
  if(!window.__lumenAloudStyle){{
    const el=document.createElement('style');
    el.textContent='::highlight(lumen-aloud){{background:#ffe08a;color:#17110a}}';
    document.head.appendChild(el);
    window.__lumenAloudStyle=el;
  }}
  const s=window.__lumenAloud;
  if(n<0||!s||!s.ranges[n]){{ CSS.highlights.delete(key); return true; }}
  const [a,b]=s.ranges[n];
  const hl=new Highlight();
  for(const [node,off] of s.nodes){{
    const len=node.nodeValue.length;
    if(off+len<=a||off>=b) continue;
    const r=document.createRange();
    r.setStart(node,Math.max(a,off)-off);
    r.setEnd(node,Math.min(b,off+len)-off);
    hl.add(r);
  }}
  CSS.highlights.set(key,hl);
  return true;
}})()"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prep_walks_text_nodes() {
        let s = prep_script();
        assert!(s.contains("TreeWalker"));
        assert!(s.contains("nodeValue"));
    }

    #[test]
    fn highlight_names_the_sentence() {
        assert!(highlight_script(Some(3)).contains("const n=3"));
        assert!(highlight_script(None).contains("const n=-1"));
        assert!(highlight_script(Some(0)).contains("lumen-aloud"));
    }
}
