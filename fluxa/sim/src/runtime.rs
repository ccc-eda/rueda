//! 排程與通道：Kahn 程序網路（KPN）。
//!
//! 每個 `task` 是一條 OS 執行緒；通道是有界 FIFO（depth），send 在滿時阻塞、
//! recv 在空時阻塞。所有阻塞都經過同一把全域鎖與 Condvar，所以可以精確偵測死結：
//! 當「所有存活任務都在等待」且自上次狀態變更後沒有人被喚醒，即為死結。
//!
//! epoch 機制：任何狀態變更（send/recv/close/任務結束/spawn）都 epoch++ 並把
//! blocked 歸零，等待者醒來後必須重新檢查並重新登記，避免「通知已送出但對方尚未醒來」
//! 造成的誤判。

use crate::ast::Program;
use crate::value::*;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};

#[derive(Debug)]
pub enum RtErr {
    /// (行號, 訊息)
    Msg(u32, String),
    /// 內部控制流：`return`
    Ret(Value),
    /// 模擬結束，任務應安靜退出
    Shutdown,
    /// 偵測到死結
    Deadlock,
}
pub type R<T> = Result<T, RtErr>;

pub fn msg<T>(line: u32, m: impl Into<String>) -> R<T> {
    Err(RtErr::Msg(line, m.into()))
}

pub struct Channel {
    pub name: String,
    pub elem: RtType,
    pub depth: usize,
    pub queue: VecDeque<Value>,
    pub closed: bool,
    pub sent: u64,
    pub received: u64,
    pub max_occ: usize,
    pub tx_owner: usize,
    pub rx_owner: usize,
}

#[derive(Default)]
pub struct World {
    pub chans: Vec<Channel>,
    pub live: usize,
    pub blocked: usize,
    pub epoch: u64,
    pub deadlock: bool,
    pub shutdown: bool,
    pub next_task: usize,
    pub finished: HashSet<usize>,
    pub waiting: HashMap<usize, String>,
    pub task_names: HashMap<usize, String>,
    pub error: Option<String>,
    pub deadlock_report: Option<String>,
    pub seq: u64,
}

impl World {
    fn bump(&mut self) {
        self.epoch += 1;
        self.blocked = 0;
        self.seq += 1;
    }
}

pub struct Shared {
    pub world: Mutex<World>,
    pub cv: Condvar,
    pub program: Arc<Program>,
    pub trace: bool,
    pub stop: AtomicBool,
    pub handles: Mutex<Vec<std::thread::JoinHandle<()>>>,
}

impl Shared {
    pub fn new(program: Arc<Program>, trace: bool) -> Arc<Shared> {
        let mut w = World { next_task: 1, live: 1, ..Default::default() }; // 任務 0 = test 主體
        w.task_names.insert(0, "test".to_string());
        Arc::new(Shared {
            world: Mutex::new(w),
            cv: Condvar::new(),
            program,
            trace,
            stop: AtomicBool::new(false),
            handles: Mutex::new(Vec::new()),
        })
    }

    pub fn lock(&self) -> MutexGuard<'_, World> {
        self.world.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    /// 阻塞直到 `f` 回傳 Some。f 在持鎖狀態下執行；回傳 None 表示尚不能前進。
    pub fn wait_until<T>(
        &self,
        task: usize,
        why: &dyn Fn(&World) -> String,
        mut f: impl FnMut(&mut World) -> Option<R<T>>,
    ) -> R<T> {
        let mut w = self.lock();
        loop {
            if w.shutdown {
                return Err(RtErr::Shutdown);
            }
            if w.deadlock {
                return Err(RtErr::Deadlock);
            }
            if let Some(r) = f(&mut w) {
                drop(w);
                self.cv.notify_all();
                return r;
            }
            let reason = why(&w);
            w.waiting.insert(task, reason);
            w.blocked += 1;
            let my_epoch = w.epoch;
            if w.blocked >= w.live {
                w.deadlock = true;
                let mut lines: Vec<String> = w
                    .waiting
                    .iter()
                    .map(|(t, r)| {
                        format!("  task `{}` (#{}) is blocked: {}", w.task_names.get(t).cloned().unwrap_or_default(), t, r)
                    })
                    .collect();
                lines.sort();
                w.deadlock_report = Some(format!(
                    "deadlock: all {} live task(s) are blocked\n{}",
                    w.live,
                    lines.join("\n")
                ));
                drop(w);
                self.cv.notify_all();
                return Err(RtErr::Deadlock);
            }
            w = self.cv.wait(w).unwrap_or_else(|e| e.into_inner());
            if w.epoch == my_epoch {
                w.blocked = w.blocked.saturating_sub(1); // 假喚醒：仍在登記中
            }
            w.waiting.remove(&task);
        }
    }

    // ------------------------------------------------------------ 任務
    pub fn alloc_task(&self, name: &str) -> usize {
        let mut w = self.lock();
        let id = w.next_task;
        w.next_task += 1;
        w.live += 1;
        w.task_names.insert(id, name.to_string());
        w.bump();
        id
    }

    pub fn launch(self: &Arc<Self>, id: usize, f: impl FnOnce(usize) -> R<()> + Send + 'static) {
        let sh = self.clone();
        let h = std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(move || {
                let r = f(id);
                if let Err(RtErr::Msg(line, m)) = r {
                    let mut w = sh.lock();
                    if w.error.is_none() {
                        let name = w.task_names.get(&id).cloned().unwrap_or_default();
                        w.error = Some(format!(
                            "{}: runtime error in task `{}`: {}",
                            sh.program.loc(line), name, m
                        ));
                    }
                    w.shutdown = true;
                    w.bump();
                    drop(w);
                    sh.stop.store(true, Ordering::Relaxed);
                    sh.cv.notify_all();
                }
                sh.task_done(id);
            })
            .expect("failed to spawn task thread");
        self.handles.lock().unwrap_or_else(|e| e.into_inner()).push(h);
    }

    pub fn task_done(&self, id: usize) {
        let mut w = self.lock();
        w.live = w.live.saturating_sub(1);
        w.finished.insert(id);
        w.bump();
        drop(w);
        self.cv.notify_all();
    }

    pub fn finish_test(&self) {
        let mut w = self.lock();
        w.shutdown = true;
        w.bump();
        drop(w);
        self.stop.store(true, Ordering::Relaxed);
        self.cv.notify_all();
    }

    pub fn join_all(&self) {
        loop {
            let h = self.handles.lock().unwrap_or_else(|e| e.into_inner()).pop();
            match h {
                Some(h) => {
                    let _ = h.join();
                }
                None => break,
            }
        }
    }

    pub fn join_children(&self, task: usize, ids: Vec<usize>) -> R<()> {
        self.wait_until(
            task,
            &|_| format!("waiting for {} child task(s) of `parallel`", ids.len()),
            |w| {
                if ids.iter().all(|i| w.finished.contains(i)) {
                    w.bump();
                    Some(Ok(()))
                } else {
                    None
                }
            },
        )
    }

    // ------------------------------------------------------------ 通道
    pub fn new_chan(&self, owner: usize, name: String, elem: RtType, depth: usize) -> usize {
        self.new_chan_owned(owner, owner, name, elem, depth)
    }

    pub fn new_chan_owned(&self, tx_owner: usize, rx_owner: usize, name: String, elem: RtType, depth: usize) -> usize {
        let mut w = self.lock();
        w.chans.push(Channel {
            name,
            elem,
            depth: depth.max(1),
            queue: VecDeque::new(),
            closed: false,
            sent: 0,
            received: 0,
            max_occ: 0,
            tx_owner,
            rx_owner,
        });
        w.chans.len() - 1
    }

    pub fn rename_chan(&self, ch: usize, name: String) {
        self.lock().chans[ch].name = name;
    }

    pub fn chan_info(&self, ch: usize) -> (RtType, usize) {
        let w = self.lock();
        (w.chans[ch].elem.clone(), w.chans[ch].depth)
    }

    pub fn send(&self, task: usize, ch: usize, v: Value, line: u32) -> R<()> {
        let (elem, name) = {
            let w = self.lock();
            (w.chans[ch].elem.clone(), w.chans[ch].name.clone())
        };
        let v = match coerce(v, &elem) {
            Ok(v) => v,
            Err(m) => return msg(line, format!("send on channel `{}` ({}): {}", name, elem, m)),
        };
        let mut pending = Some(v);
        let trace = self.trace;
        self.wait_until(
            task,
            &|w| format!("send on `{}` (FIFO full, depth {})", w.chans[ch].name, w.chans[ch].depth),
            |w| {
                if w.chans[ch].tx_owner != task {
                    return Some(msg(line, format!(
                        "task does not own the tx endpoint of channel `{}` (moved by spawn, or aliased)",
                        w.chans[ch].name
                    )));
                }
                if w.chans[ch].closed {
                    return Some(msg(line, format!("send on closed channel `{}`", w.chans[ch].name)));
                }
                if w.chans[ch].queue.len() < w.chans[ch].depth {
                    let v = pending.take().unwrap();
                    if trace {
                        eprintln!("[{:>6}] {:<24} send {}", w.seq, w.chans[ch].name, v);
                    }
                    let c = &mut w.chans[ch];
                    c.queue.push_back(v);
                    c.sent += 1;
                    c.max_occ = c.max_occ.max(c.queue.len());
                    w.bump();
                    Some(Ok(()))
                } else {
                    None
                }
            },
        )
    }

    /// None = 通道已關閉且為空
    pub fn recv(&self, task: usize, ch: usize, line: u32) -> R<Option<Value>> {
        let trace = self.trace;
        self.wait_until(
            task,
            &|w| format!("recv on `{}` (FIFO empty)", w.chans[ch].name),
            |w| {
                if w.chans[ch].rx_owner != task {
                    return Some(msg(line, format!(
                        "task does not own the rx endpoint of channel `{}` (moved by spawn, or aliased)",
                        w.chans[ch].name
                    )));
                }
                if let Some(v) = w.chans[ch].queue.pop_front() {
                    w.chans[ch].received += 1;
                    if trace {
                        eprintln!("[{:>6}] {:<24} recv {}", w.seq, w.chans[ch].name, v);
                    }
                    w.bump();
                    Some(Ok(Some(v)))
                } else if w.chans[ch].closed {
                    Some(Ok(None))
                } else {
                    None
                }
            },
        )
    }

    pub fn close(&self, task: usize, ch: usize, line: u32) -> R<()> {
        let mut w = self.lock();
        if w.chans[ch].tx_owner != task {
            return msg(line, format!("task does not own the tx endpoint of channel `{}`", w.chans[ch].name));
        }
        w.chans[ch].closed = true;
        w.bump();
        drop(w);
        self.cv.notify_all();
        Ok(())
    }

    /// 結束任務時關閉其仍擁有的 Tx 端點（等同硬體的 end-of-stream）
    pub fn close_owned_tx(&self, task: usize, v: &Value) {
        match v {
            Value::Tx(ch) => {
                let mut w = self.lock();
                if w.chans[*ch].tx_owner == task && !w.chans[*ch].closed {
                    w.chans[*ch].closed = true;
                    w.bump();
                    drop(w);
                    self.cv.notify_all();
                }
            }
            Value::Array(xs) | Value::Tuple(xs) => xs.iter().for_each(|x| self.close_owned_tx(task, x)),
            _ => {}
        }
    }

    /// 線性所有權轉移：端點必須由 `from` 擁有，轉給 `to`
    pub fn transfer(&self, v: &Value, from: usize, to: usize, line: u32) -> R<()> {
        match v {
            Value::Tx(ch) | Value::Rx(ch) => {
                let is_tx = matches!(v, Value::Tx(_));
                let mut w = self.lock();
                let name = w.chans[*ch].name.clone();
                let owner = if is_tx { &mut w.chans[*ch].tx_owner } else { &mut w.chans[*ch].rx_owner };
                if *owner != from {
                    return msg(line, format!(
                        "{} endpoint of channel `{}` was already moved (channel endpoints are linear)",
                        if is_tx { "tx" } else { "rx" },
                        name
                    ));
                }
                *owner = to;
                Ok(())
            }
            Value::Array(xs) | Value::Tuple(xs) => {
                for x in xs {
                    self.transfer(x, from, to, line)?;
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    pub fn stats_report(&self) -> String {
        let w = self.lock();
        let mut s = String::new();
        s.push_str(&format!("  {:<28} {:>5} {:>8} {:>8} {:>7}\n", "channel", "depth", "sent", "recv", "max-occ"));
        for c in &w.chans {
            if c.sent == 0 && c.received == 0 {
                continue;
            }
            s.push_str(&format!(
                "  {:<28} {:>5} {:>8} {:>8} {:>7}\n",
                c.name, c.depth, c.sent, c.received, c.max_occ
            ));
        }
        s
    }
}
