//! Read aloud.

use std::sync::OnceLock;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::time::Duration;

use windows::Win32::Media::Speech::{ISpVoice, SPRS_DONE, SPVOICESTATUS, SVSFlagsAsync, SVSFPurgeBeforeSpeak, SVSFIsNotXML};
use windows::Win32::System::Com::{CLSCTX_ALL, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx};
use windows::core::{GUID, PCWSTR};

const POLL: Duration = Duration::from_millis(50);

const CLSID_SPVOICE: GUID = GUID::from_u128(0x96749377_3391_11D2_9EE3_00C04F797396);

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
    let mut said = false;
    loop {
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

fn say(v: &ISpVoice, text: &str) {
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    const FLAGS: u32 = (SVSFlagsAsync.0 | SVSFPurgeBeforeSpeak.0 | SVSFIsNotXML.0) as u32;
    unsafe {
        let _ = v.Speak(PCWSTR(wide.as_ptr()), FLAGS, None);
    }
    std::thread::sleep(Duration::from_millis(30));
}

fn finished(v: &ISpVoice) -> bool {
    let mut st = SPVOICESTATUS::default();
    unsafe { v.GetStatus(&mut st, std::ptr::null_mut()).is_ok() && st.dwRunningState == SPRS_DONE.0 as u32 }
}

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

#[cfg(test)]
mod live {
    use super::*;

    #[test]
    #[ignore = "needs the Windows speech engine"]
    fn the_system_voice_comes_up() {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let v = CoCreateInstance::<_, ISpVoice>(&CLSID_SPVOICE, None, CLSCTX_ALL);
            assert!(v.is_ok(), "no voice on this machine: {:?}", v.err());
        }
    }
}
