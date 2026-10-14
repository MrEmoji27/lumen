//! Who uses what, and the knobs that cap it.
//!
//! - Job: the whole browser tree lives in one Windows Job Object. It gives a hard CPU
//!   cap and kills every WebView2 process when Lumen dies. No orphans.
//! - sample(): which processes belong to which tab, and their memory and CPU time.
//! - Network: bytes per tab and a bandwidth cap, through the DevTools protocol.

use std::{
    cell::RefCell,
    collections::HashMap,
    rc::Rc,
};

use tao::event_loop::EventLoopProxy;
use webview2_com::{
    CallDevToolsProtocolMethodCompletedHandler, DevToolsProtocolEventReceivedEventHandler,
    GetProcessExtendedInfosCompletedHandler, Microsoft::Web::WebView2::Win32::*,
};
use windows::{
    Win32::{
        Foundation::{CloseHandle, FILETIME, HANDLE},
        System::{
            Com::CoTaskMemFree,
            JobObjects::{
                AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_CPU_RATE_CONTROL_ENABLE,
                JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
                JOBOBJECT_CPU_RATE_CONTROL_INFORMATION, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
                JobObjectCpuRateControlInformation, JobObjectExtendedLimitInformation,
                SetInformationJobObject,
            },
            ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX2},
            Threading::{GetCurrentProcess, GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION},
        },
    },
    core::{HSTRING, Interface, PCWSTR, PWSTR},
};
use wry::{WebView, WebViewExtWindows};

use crate::engine::Ev;

// --------------------------------------------------------------------- job

pub struct Job(HANDLE);

impl Job {
    /// Put Lumen in a job before WebView2 starts. Its processes inherit the job.
    /// None if Windows refuses (some launchers hold us in a job that forbids it).
    pub fn adopt_self() -> Option<Job> {
        unsafe {
            let job = CreateJobObjectW(None, PCWSTR::null()).ok()?;
            let mut lim = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            lim.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let _ = SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &lim as *const _ as _,
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            if AssignProcessToJobObject(job, GetCurrentProcess()).is_err() {
                let _ = CloseHandle(job);
                return None;
            }
            Some(Job(job))
        }
    }

    /// Hard cap on CPU for the whole tree, percent of all cores. None lifts it.
    pub fn cap_cpu(&self, pct: Option<u32>) {
        let mut info = JOBOBJECT_CPU_RATE_CONTROL_INFORMATION::default();
        if let Some(p) = pct {
            info.ControlFlags = JOB_OBJECT_CPU_RATE_CONTROL_ENABLE | JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP;
            // Units of 1/100 percent.
            info.Anonymous.CpuRate = p.clamp(1, 100) * 100;
        }
        unsafe {
            let _ = SetInformationJobObject(
                self.0,
                JobObjectCpuRateControlInformation,
                &info as *const _ as _,
                size_of::<JOBOBJECT_CPU_RATE_CONTROL_INFORMATION>() as u32,
            );
        }
    }
}

// ------------------------------------------------------------- per process

/// Private working set: the number Task Manager calls "Memory".
pub fn private_ws(h: HANDLE) -> u64 {
    let mut c = PROCESS_MEMORY_COUNTERS_EX2::default();
    let ok = unsafe {
        GetProcessMemoryInfo(
            h,
            &mut c as *mut _ as *mut PROCESS_MEMORY_COUNTERS,
            size_of::<PROCESS_MEMORY_COUNTERS_EX2>() as u32,
        )
    };
    if ok.is_ok() { c.PrivateWorkingSetSize as u64 } else { 0 }
}

/// Total CPU time, kernel plus user, in 100 ns units.
fn cpu_time(h: HANDLE) -> u64 {
    let (mut a, mut b, mut k, mut u) = (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
    if unsafe { GetProcessTimes(h, &mut a, &mut b, &mut k, &mut u) }.is_err() {
        return 0;
    }
    let ft = |f: FILETIME| ((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64;
    ft(k) + ft(u)
}

fn read(pid: u32) -> (u64, u64) {
    unsafe {
        match OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
            Ok(h) => {
                let r = (private_ws(h), cpu_time(h));
                let _ = CloseHandle(h);
                r
            }
            Err(_) => (0, 0),
        }
    }
}

/// One process, raw. main.rs turns a pair of these into percentages.
#[derive(Debug, Clone)]
pub struct Proc {
    pub pid: u32,
    pub kind: &'static str,
    /// Main-frame ids of every page this process works for.
    pub frames: Vec<u32>,
    pub mem: u64,
    pub cpu: u64,
}

fn kind_name(k: COREWEBVIEW2_PROCESS_KIND) -> &'static str {
    match k {
        COREWEBVIEW2_PROCESS_KIND_BROWSER => "browser",
        COREWEBVIEW2_PROCESS_KIND_RENDERER => "renderer",
        COREWEBVIEW2_PROCESS_KIND_GPU => "gpu",
        COREWEBVIEW2_PROCESS_KIND_UTILITY => "utility",
        _ => "other",
    }
}

/// The page a frame belongs to: walk up to the top frame and take its id.
unsafe fn top_frame(f: ICoreWebView2FrameInfo) -> Option<u32> {
    let mut cur = f.cast::<ICoreWebView2FrameInfo2>().ok()?;
    for _ in 0..32 {
        match unsafe { cur.ParentFrameInfo() } {
            Ok(p) => match p.cast::<ICoreWebView2FrameInfo2>() {
                Ok(p2) => cur = p2,
                Err(_) => break,
            },
            Err(_) => break,
        }
    }
    let mut id = 0u32;
    unsafe { cur.FrameId(&mut id) }.ok()?;
    Some(id)
}

/// Ask WebView2 which process serves which page. The answer arrives as Ev::Usage.
pub fn sample(env: &ICoreWebView2Environment, proxy: EventLoopProxy<Ev>) {
    let Ok(env13) = env.cast::<ICoreWebView2Environment13>() else { return };
    let handler = GetProcessExtendedInfosCompletedHandler::create(Box::new(move |hr, list| {
        let mut out = Vec::new();
        // Lumen itself: the host process.
        let me = unsafe { GetCurrentProcess() };
        out.push(Proc { pid: std::process::id(), kind: "host", frames: vec![], mem: private_ws(me), cpu: cpu_time(me) });
        if hr.is_ok()
            && let Some(list) = list
        {
            unsafe {
                let mut n = 0u32;
                let _ = list.Count(&mut n);
                for i in 0..n {
                    let Ok(ext) = list.GetValueAtIndex(i) else { continue };
                    let Ok(info) = ext.ProcessInfo() else { continue };
                    let (mut pid, mut kind) = (0i32, COREWEBVIEW2_PROCESS_KIND::default());
                    let _ = info.ProcessId(&mut pid);
                    let _ = info.Kind(&mut kind);
                    let mut frames = Vec::new();
                    if let Ok(fl) = ext.AssociatedFrameInfos()
                        && let Ok(it) = fl.GetIterator()
                    {
                        let mut has = Default::default();
                        let _ = it.HasCurrent(&mut has);
                        while has.as_bool() {
                            if let Ok(f) = it.GetCurrent()
                                && let Some(top) = top_frame(f)
                                && !frames.contains(&top)
                            {
                                frames.push(top);
                            }
                            if it.MoveNext(&mut has).is_err() {
                                break;
                            }
                        }
                    }
                    let (mem, cpu) = read(pid as u32);
                    out.push(Proc { pid: pid as u32, kind: kind_name(kind), frames, mem, cpu });
                }
            }
        }
        let _ = proxy.send_event(Ev::Usage(out));
        Ok(())
    }));
    unsafe {
        let _ = env13.GetProcessExtendedInfos(&handler);
    }
}

/// Main-frame id of a page. Ties processes back to tabs.
pub fn frame_id(wv: &WebView) -> Option<u32> {
    let c20 = wv.webview().cast::<ICoreWebView2_20>().ok()?;
    let mut id = 0u32;
    unsafe { c20.FrameId(&mut id) }.ok()?;
    Some(id)
}

// ----------------------------------------------------------------- network

fn cdp(wv: &WebView, method: &str, params: &str) {
    unsafe {
        let _ = wv.webview().CallDevToolsProtocolMethod(
            &HSTRING::from(method),
            &HSTRING::from(params),
            &CallDevToolsProtocolMethodCompletedHandler::create(Box::new(|_, _| Ok(()))),
        );
    }
}

/// Cap a page's bandwidth, KB/s. None lifts the cap.
pub fn throttle(wv: &WebView, kbps: Option<u32>) {
    match kbps {
        Some(k) => {
            let bps = k as u64 * 1024;
            cdp(wv, "Network.enable", "{}");
            cdp(
                wv,
                "Network.emulateNetworkConditions",
                &format!(r#"{{"offline":false,"latency":0,"downloadThroughput":{bps},"uploadThroughput":{bps}}}"#),
            );
        }
        None => cdp(
            wv,
            "Network.emulateNetworkConditions",
            r#"{"offline":false,"latency":0,"downloadThroughput":-1,"uploadThroughput":-1}"#,
        ),
    }
}

/// Bytes received per tab since the last read. Fed by DevTools events.
pub type NetBytes = Rc<RefCell<HashMap<u64, u64>>>;

pub struct NetWatch {
    recv: ICoreWebView2DevToolsProtocolEventReceiver,
    token: i64,
}

/// Count bytes for one tab. Only while the Control page is open: the events cost.
pub fn watch(wv: &WebView, tab: u64, sink: NetBytes) -> Option<NetWatch> {
    cdp(wv, "Network.enable", "{}");
    unsafe {
        let recv = wv.webview().GetDevToolsProtocolEventReceiver(&HSTRING::from("Network.dataReceived")).ok()?;
        let mut token = 0i64;
        recv.add_DevToolsProtocolEventReceived(
            &DevToolsProtocolEventReceivedEventHandler::create(Box::new(move |_, args| {
                let Some(args) = args else { return Ok(()) };
                let mut p = PWSTR::null();
                args.ParameterObjectAsJson(&mut p)?;
                if !p.is_null() {
                    let n = encoded_len(&p.to_string().unwrap_or_default());
                    CoTaskMemFree(Some(p.0 as *const _));
                    *sink.borrow_mut().entry(tab).or_default() += n;
                }
                Ok(())
            })),
            &mut token,
        )
        .ok()?;
        Some(NetWatch { recv, token })
    }
}

pub fn unwatch(wv: Option<&WebView>, w: NetWatch, keep_enabled: bool) {
    unsafe {
        let _ = w.recv.remove_DevToolsProtocolEventReceived(w.token);
    }
    if let Some(wv) = wv
        && !keep_enabled
    {
        cdp(wv, "Network.disable", "{}");
    }
}

/// Pull "encodedDataLength": N out of the event JSON without a full parse.
fn encoded_len(json: &str) -> u64 {
    const KEY: &str = "\"encodedDataLength\":";
    json.find(KEY)
        .map(|i| &json[i + KEY.len()..])
        .map(|rest| rest.trim_start().chars().take_while(|c| c.is_ascii_digit()).collect::<String>())
        .and_then(|d| d.parse().ok())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    #[test]
    fn reads_encoded_length() {
        assert_eq!(super::encoded_len(r#"{"requestId":"1","dataLength":10,"encodedDataLength":1234}"#), 1234);
        assert_eq!(super::encoded_len(r#"{"encodedDataLength": 7,"x":1}"#), 7);
        assert_eq!(super::encoded_len("{}"), 0);
    }
}
