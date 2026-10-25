//! Who uses what, and the knobs that cap it.

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
            Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS},
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

pub struct Job(HANDLE);

impl Job {
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

    pub fn cap_cpu(&self, pct: Option<u32>) {
        let mut info = JOBOBJECT_CPU_RATE_CONTROL_INFORMATION::default();
        if let Some(p) = pct {
            info.ControlFlags = JOB_OBJECT_CPU_RATE_CONTROL_ENABLE | JOB_OBJECT_CPU_RATE_CONTROL_HARD_CAP;
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

pub fn cpu_time(h: HANDLE) -> u64 {
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

fn born(pid: u32) -> Option<u64> {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let (mut c, mut e, mut k, mut u) = (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
        let ok = GetProcessTimes(h, &mut c, &mut e, &mut k, &mut u).is_ok();
        let _ = CloseHandle(h);
        ok.then(|| ((c.dwHighDateTime as u64) << 32) | c.dwLowDateTime as u64)
    }
}

pub fn tree() -> Vec<(u32, String)> {
    let mut all = Vec::new();
    unsafe {
        if let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) {
            let mut e = PROCESSENTRY32W { dwSize: size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
            let mut ok = Process32FirstW(snap, &mut e).is_ok();
            while ok {
                let n = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
                all.push((e.th32ProcessID, e.th32ParentProcessID, String::from_utf16_lossy(&e.szExeFile[..n])));
                ok = Process32NextW(snap, &mut e).is_ok();
            }
            let _ = CloseHandle(snap);
        }
    }
    descendants(std::process::id(), &all, born)
}

fn descendants(root: u32, all: &[(u32, u32, String)], born: impl Fn(u32) -> Option<u64>) -> Vec<(u32, String)> {
    let name = |pid| all.iter().find(|p| p.0 == pid).map(|p| p.2.clone()).unwrap_or_default();
    let mut out = vec![(root, name(root), born(root))];
    let mut i = 0;
    while i < out.len() {
        let (parent, parent_born) = (out[i].0, out[i].2);
        for (pid, ppid, name) in all {
            if *ppid != parent || *pid == parent || out.iter().any(|o| o.0 == *pid) {
                continue;
            }
            let b = born(*pid);
            if let (Some(c), Some(p)) = (b, parent_born)
                && c >= p
            {
                out.push((*pid, name.clone(), b));
            }
        }
        i += 1;
    }
    out.into_iter().map(|(pid, name, _)| (pid, name)).collect()
}

#[derive(Debug, Clone)]
pub struct Proc {
    pub pid: u32,
    pub kind: &'static str,
    pub env: String,
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

fn rest_of_tree(out: &mut Vec<Proc>) {
    let me = std::process::id();
    for (pid, name) in tree() {
        if out.iter().any(|p| p.pid == pid) {
            continue;
        }
        let kind = if pid == me {
            "host"
        } else if name.eq_ignore_ascii_case("msedgewebview2.exe") {
            "other"
        } else {
            "helper"
        };
        let (mem, cpu) = read(pid);
        out.push(Proc { pid, kind, env: String::new(), frames: vec![], mem, cpu });
    }
}

pub fn sample(envs: Vec<(String, ICoreWebView2Environment)>, proxy: EventLoopProxy<Ev>) {
    let envs: Vec<_> = envs.into_iter().filter_map(|(k, e)| e.cast::<ICoreWebView2Environment13>().ok().map(|e| (k, e))).collect();
    let acc = Rc::new(RefCell::new((envs.len(), Vec::<Proc>::new())));
    let done = {
        let acc = acc.clone();
        Rc::new(move || {
            let mut a = acc.borrow_mut();
            a.0 = a.0.saturating_sub(1);
            if a.0 == 0 {
                let mut out = std::mem::take(&mut a.1);
                rest_of_tree(&mut out);
                let _ = proxy.send_event(Ev::Usage(out));
            }
        })
    };
    if envs.is_empty() {
        acc.borrow_mut().0 = 1;
        done();
        return;
    }
    for (key, env13) in envs {
        let (acc, done2) = (acc.clone(), done.clone());
        let handler = GetProcessExtendedInfosCompletedHandler::create(Box::new(move |hr, list| {
            let found = if hr.is_ok() { list.map(|l| unsafe { procs_of(&l, &key) }).unwrap_or_default() } else { Vec::new() };
            acc.borrow_mut().1.extend(found);
            done2();
            Ok(())
        }));
        if unsafe { env13.GetProcessExtendedInfos(&handler) }.is_err() {
            done();
        }
    }
}

unsafe fn procs_of(list: &ICoreWebView2ProcessExtendedInfoCollection, key: &str) -> Vec<Proc> {
    let mut out = Vec::new();
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
            out.push(Proc { pid: pid as u32, kind: kind_name(kind), env: key.to_string(), frames, mem, cpu });
        }
    }
    out
}

pub fn frame_id(wv: &WebView) -> Option<u32> {
    let c20 = wv.webview().cast::<ICoreWebView2_20>().ok()?;
    let mut id = 0u32;
    unsafe { c20.FrameId(&mut id) }.ok()?;
    Some(id)
}

fn cdp(wv: &WebView, method: &str, params: &str) {
    unsafe {
        let _ = wv.webview().CallDevToolsProtocolMethod(
            &HSTRING::from(method),
            &HSTRING::from(params),
            &CallDevToolsProtocolMethodCompletedHandler::create(Box::new(|_, _| Ok(()))),
        );
    }
}

fn enable_net(wv: &WebView, agent: bool) {
    if !agent {
        cdp(wv, "Network.enable", r#"{"maxTotalBufferSize":0,"maxResourceBufferSize":0}"#);
    }
}

pub fn throttle(wv: &WebView, kbps: Option<u32>, agent: bool) {
    match kbps {
        Some(k) => {
            let bps = k as u64 * 1024;
            enable_net(wv, agent);
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

pub type NetBytes = Rc<RefCell<HashMap<u64, u64>>>;

pub struct NetWatch {
    subs: Vec<(ICoreWebView2DevToolsProtocolEventReceiver, i64)>,
}

const NET_EVENTS: [&str; 4] =
    ["Network.dataReceived", "Network.loadingFinished", "Network.loadingFailed", "Network.webSocketFrameReceived"];

pub fn watch(wv: &WebView, tab: u64, sink: NetBytes, agent: bool) -> Option<NetWatch> {
    enable_net(wv, agent);
    let open: Rc<RefCell<HashMap<String, u64>>> = Rc::default();
    let mut subs = Vec::new();
    for ev in NET_EVENTS {
        let (sink, open) = (sink.clone(), open.clone());
        let sub = unsafe {
            let recv = wv.webview().GetDevToolsProtocolEventReceiver(&HSTRING::from(ev)).ok();
            let mut token = 0i64;
            recv.filter(|r| {
                r.add_DevToolsProtocolEventReceived(
                    &DevToolsProtocolEventReceivedEventHandler::create(Box::new(move |_, args| {
                        let Some(args) = args else { return Ok(()) };
                        let mut p = PWSTR::null();
                        args.ParameterObjectAsJson(&mut p)?;
                        if !p.is_null() {
                            let v: serde_json::Value = serde_json::from_str(&p.to_string().unwrap_or_default()).unwrap_or_default();
                            CoTaskMemFree(Some(p.0 as *const _));
                            let n = bytes_of(ev, &v, &mut open.borrow_mut());
                            if n > 0 {
                                *sink.borrow_mut().entry(tab).or_default() += n;
                            }
                        }
                        Ok(())
                    })),
                    &mut token,
                )
                .is_ok()
            })
            .map(|r| (r, token))
        };
        subs.extend(sub);
    }
    (!subs.is_empty()).then_some(NetWatch { subs })
}

pub fn unwatch(wv: Option<&WebView>, w: NetWatch, keep_enabled: bool) {
    for (recv, token) in w.subs {
        unsafe {
            let _ = recv.remove_DevToolsProtocolEventReceived(token);
        }
    }
    if let Some(wv) = wv
        && !keep_enabled
    {
        cdp(wv, "Network.disable", "{}");
    }
}

fn bytes_of(event: &str, v: &serde_json::Value, open: &mut HashMap<String, u64>) -> u64 {
    let id = v["requestId"].as_str().unwrap_or_default();
    match event {
        "Network.dataReceived" => {
            let n = v["encodedDataLength"].as_u64().unwrap_or(0);
            *open.entry(id.to_string()).or_default() += n;
            n
        }
        "Network.loadingFinished" => {
            let total = v["encodedDataLength"].as_f64().unwrap_or(0.0).max(0.0) as u64;
            total.saturating_sub(open.remove(id).unwrap_or(0))
        }
        "Network.loadingFailed" => {
            open.remove(id);
            0
        }
        "Network.webSocketFrameReceived" => {
            let r = &v["response"];
            let len = r["payloadData"].as_str().map_or(0, str::len) as u64;
            if r["opcode"].as_u64() == Some(2) { len * 3 / 4 } else { len }
        }
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use serde_json::json;

    use super::{bytes_of, descendants};

    #[test]
    fn bytes_count_chunks_then_the_rest_at_the_finish() {
        let mut open = HashMap::new();
        let mut sum = 0;
        for (ev, v) in [
            ("Network.dataReceived", json!({"requestId": "1", "dataLength": 4000, "encodedDataLength": 0})),
            ("Network.dataReceived", json!({"requestId": "1", "dataLength": 4000, "encodedDataLength": 300})),
            ("Network.dataReceived", json!({"requestId": "2", "dataLength": 10, "encodedDataLength": 0})),
            ("Network.loadingFinished", json!({"requestId": "1", "encodedDataLength": 1234.0})),
            ("Network.loadingFailed", json!({"requestId": "2"})),
            ("Network.webSocketFrameReceived", json!({"requestId": "3", "response": {"opcode": 1, "payloadData": "hello"}})),
            ("Network.webSocketFrameReceived", json!({"requestId": "3", "response": {"opcode": 2, "payloadData": "AAAA"}})),
        ] {
            sum += bytes_of(ev, &v, &mut open);
        }
        assert_eq!(sum, 1234 + 5 + 3);
        assert!(open.is_empty());
    }

    #[test]
    fn tree_takes_children_of_children_but_not_strangers_on_a_reused_pid() {
        let all = vec![
            (10, 1, "lumen.exe".to_string()),
            (11, 10, "msedgewebview2.exe".to_string()),
            (12, 10, "aria2c.exe".to_string()),
            (13, 11, "msedgewebview2.exe".to_string()),
            (99, 12, "stranger.exe".to_string()),
            (50, 1, "other.exe".to_string()),
        ];
        let born = |pid: u32| Some(match pid { 10 => 100, 11 => 110, 12 => 120, 13 => 130, 99 => 50, _ => 1 });
        let got: Vec<u32> = descendants(10, &all, born).into_iter().map(|p| p.0).collect();
        assert_eq!(got, vec![10, 11, 12, 13]);
    }
}
