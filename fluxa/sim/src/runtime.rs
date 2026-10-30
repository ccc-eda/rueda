//! 排程與通道：Kahn 程序網路（KPN）。
//!
//! 通道是有界 FIFO（depth），send 在滿時阻塞、recv 在空時阻塞。兩種排程方式，
//! 結果相同（KPN 的輸出與排程無關）：
//!
//! - 原生（預設）：每個 `task` 是一條 OS 執行緒。所有阻塞都經過同一把全域鎖與 Condvar，
//!   所以可以精確偵測死結：當「所有存活任務都在等待」且自上次狀態變更後沒有人被喚醒，即為死結。
//! - 協作式（`coop`，WebAssembly 沒有執行緒時用）：所有任務是同一條執行緒上的 future，
//!   等待時讓出（Pending），由 [`Shared::run_coop`] 輪流執行；一整輪沒有任何狀態變更即為死結。
//!   `print` 的輸出收進緩衝區（[`Shared::take_output`]），不直接印出。
//!
//! epoch 機制：任何狀態變更（send/recv/close/任務結束/spawn）都 epoch++ 並把
//! blocked 歸零，等待者醒來後必須重新檢查並重新登記，避免「通知已送出但對方尚未醒來」
//! 造成的誤判。

use crate::ast::Program;
use crate::interp::BoxFut;
use crate::value::*;
use std::collections::{HashMap, HashSet, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

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
    pub(crate) fn bump(&mut self) {
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
    /// 協作式排程（沒有執行緒）：等待點讓出，任務由 `run_coop` 輪流執行
    pub coop: bool,
    /// 協作式：剛 spawn、尚未交給 `run_coop` 的任務
    spawned: Mutex<Vec<(usize, BoxFut<'static, R<()>>)>>,
    /// 協作式：`print` 與 `--trace` 的輸出
    out: Mutex<String>,
    /// 以硬體（合成後的網表）取代的任務：spawn 時先問它
    pub hw: Option<Arc<dyn HwProvider>>,
}

/// 硬體任務的通道 token：一個值，或串流結束（close）
#[derive(Clone, Debug, PartialEq)]
pub enum HwTok {
    Data(i128),
    Close,
}

/// 一個以硬體執行的任務（例如合成後的網表在閘級模擬器裡）：每呼叫一次走一個時脈
pub trait HwTask: Send {
    /// `offer`：每條輸入通道提供的 token；`room`：每條輸出通道還能不能收。
    /// 回傳：哪些輸入 token 被收下、每條輸出通道送出了什麼、內部狀態有沒有改變
    fn cycle(&mut self, offer: &[Option<HwTok>], room: &[bool]) -> (Vec<bool>, Vec<Option<HwTok>>, bool);
}

/// 決定哪些任務以硬體執行：`start` 對要取代的任務回傳 Some（參數依宣告順序：名稱與值）
pub trait HwProvider: Send + Sync {
    fn start(&self, task: &str, args: &[(String, Value)]) -> Option<Result<Box<dyn HwTask>, String>>;
}

impl Shared {
    pub fn new(program: Arc<Program>, trace: bool, coop: bool) -> Arc<Shared> { Shared::with_hw(program, trace, coop, None) }

    /// 同 `new`，另外指定以硬體執行的任務
    pub fn with_hw(program: Arc<Program>, trace: bool, coop: bool, hw: Option<Arc<dyn HwProvider>>) -> Arc<Shared> {
        let mut w = World { next_task: 1, live: 1, ..Default::default() }; // 任務 0 = test 主體
        w.task_names.insert(0, "test".to_string());
        Arc::new(Shared {
            world: Mutex::new(w),
            cv: Condvar::new(),
            program,
            trace,
            stop: AtomicBool::new(false),
            handles: Mutex::new(Vec::new()),
            coop,
            spawned: Mutex::new(Vec::new()),
            out: Mutex::new(String::new()),
            hw,
        })
    }

    /// `print`：原生印到 stdout，協作式收進緩衝區
    pub fn print(&self, line: &str) {
        if self.coop {
            let mut o = self.out.lock().unwrap_or_else(|e| e.into_inner());
            o.push_str(line);
            o.push('\n');
        } else {
            println!("{}", line);
        }
    }

    /// `--trace` 的一行：原生印到 stderr，協作式收進緩衝區
    fn trace_line(&self, line: String) {
        if self.coop {
            self.print(&line);
        } else {
            eprintln!("{}", line);
        }
    }

    /// 協作式：到目前為止的輸出（並清空）
    pub fn take_output(&self) -> String {
        std::mem::take(&mut *self.out.lock().unwrap_or_else(|e| e.into_inner()))
    }

    pub fn lock(&self) -> MutexGuard<'_, World> {
        self.world.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    /// 等到 `f` 回傳 Some。f 在持鎖狀態下執行；回傳 None 表示尚不能前進。
    /// 原生：阻塞這條執行緒；協作式：登記等待原因後讓出，由 `run_coop` 之後再試。
    pub async fn wait_until<T>(
        &self,
        task: usize,
        why: &(dyn Fn(&World) -> String + Sync),
        mut f: impl FnMut(&mut World) -> Option<R<T>>,
    ) -> R<T> {
        if !self.coop {
            return self.wait_blocking(task, why, f);
        }
        loop {
            {
                let mut w = self.lock();
                if w.shutdown {
                    return Err(RtErr::Shutdown);
                }
                if w.deadlock {
                    return Err(RtErr::Deadlock);
                }
                if let Some(r) = f(&mut w) {
                    w.waiting.remove(&task);
                    return r;
                }
                let reason = why(&w);
                w.waiting.insert(task, reason);
            }
            YieldOnce(false).await;
        }
    }

    fn wait_blocking<T>(
        &self,
        task: usize,
        why: &(dyn Fn(&World) -> String + Sync),
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

    /// 啟動任務 `id`：原生開一條執行緒跑這個 future，協作式交給 `run_coop`
    pub fn launch(self: &Arc<Self>, id: usize, fut: impl Future<Output = R<()>> + Send + 'static) {
        if self.coop {
            self.spawned.lock().unwrap_or_else(|e| e.into_inner()).push((id, Box::pin(fut)));
            return;
        }
        let sh = self.clone();
        let h = std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(move || {
                let r = block_on(fut);
                sh.task_finished(id, r);
            })
            .expect("failed to spawn task thread");
        self.handles.lock().unwrap_or_else(|e| e.into_inner()).push(h);
    }

    /// 任務結束：記下第一個執行期錯誤（並結束模擬），然後 `task_done`
    fn task_finished(&self, id: usize, r: R<()>) {
        if let Err(RtErr::Msg(line, m)) = r {
            let mut w = self.lock();
            if w.error.is_none() {
                let name = w.task_names.get(&id).cloned().unwrap_or_default();
                w.error = Some(format!(
                    "{}: runtime error in task `{}`: {}",
                    self.program.loc(line), name, m
                ));
            }
            w.shutdown = true;
            w.bump();
            drop(w);
            self.stop.store(true, Ordering::Relaxed);
            self.cv.notify_all();
        }
        self.task_done(id);
    }

    /// 協作式排程：輪流執行 test 主體 `main` 與所有任務，直到 `main` 結束。
    /// 一整輪沒有任何狀態變更（epoch 不變）而 `main` 還沒結束 = 所有任務都在等待 = 死結。
    pub fn run_coop<'a, T>(&self, mut main: Pin<Box<dyn Future<Output = R<T>> + 'a>>) -> R<T> {
        let waker = noop_waker();
        let mut cx = Context::from_waker(&waker);
        let mut tasks: Vec<(usize, BoxFut<'static, R<()>>)> = Vec::new();
        loop {
            let epoch = self.lock().epoch;
            if let Poll::Ready(r) = main.as_mut().poll(&mut cx) {
                return r;
            }
            let mut k = 0;
            loop {
                tasks.extend(self.spawned.lock().unwrap_or_else(|e| e.into_inner()).drain(..));
                if k >= tasks.len() {
                    break;
                }
                match tasks[k].1.as_mut().poll(&mut cx) {
                    Poll::Ready(r) => {
                        let (id, _) = tasks.remove(k);
                        self.task_finished(id, r);
                    }
                    Poll::Pending => k += 1,
                }
            }
            let mut w = self.lock();
            if w.epoch == epoch {
                if w.deadlock {
                    // 已宣告死結，而 main 仍未結束（不應發生）
                    return Err(RtErr::Deadlock);
                }
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
                w.deadlock = true;
            }
        }
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

    pub async fn join_children(&self, task: usize, ids: Vec<usize>) -> R<()> {
        let n = ids.len();
        self.wait_until(
            task,
            &|_| format!("waiting for {} child task(s) of `parallel`", n),
            |w| {
                if ids.iter().all(|i| w.finished.contains(i)) {
                    w.bump();
                    Some(Ok(()))
                } else {
                    None
                }
            },
        )
        .await
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

    pub async fn send(&self, task: usize, ch: usize, v: Value, line: u32) -> R<()> {
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
                        self.trace_line(format!("[{:>6}] {:<24} send {}", w.seq, w.chans[ch].name, v));
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
        .await
    }

    /// None = 通道已關閉且為空
    pub async fn recv(&self, task: usize, ch: usize, line: u32) -> R<Option<Value>> {
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
                        self.trace_line(format!("[{:>6}] {:<24} recv {}", w.seq, w.chans[ch].name, v));
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
        .await
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

/// 讓出一次：第一次 poll 回傳 Pending，之後 Ready（協作式的等待點）
struct YieldOnce(bool);

impl Future for YieldOnce {
    type Output = ();
    fn poll(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<()> {
        if self.0 {
            Poll::Ready(())
        } else {
            self.0 = true;
            Poll::Pending
        }
    }
}

/// 什麼都不做的 waker：排程器自己輪流 poll，不需要被喚醒
fn noop_waker() -> Waker {
    fn clone(_: *const ()) -> RawWaker {
        RawWaker::new(std::ptr::null(), &VTABLE)
    }
    fn noop(_: *const ()) {}
    static VTABLE: RawWakerVTable = RawWakerVTable::new(clone, noop, noop, noop);
    // SAFETY: every function of the vtable ignores the (null) data pointer
    unsafe { Waker::from_raw(RawWaker::new(std::ptr::null(), &VTABLE)) }
}

/// 原生：在這條執行緒上跑完一個 future（等待點直接阻塞，所以一次 poll 就結束）
pub fn block_on<T>(fut: impl Future<Output = T>) -> T {
    let waker = noop_waker();
    let mut cx = Context::from_waker(&waker);
    let mut fut = std::pin::pin!(fut);
    loop {
        if let Poll::Ready(v) = fut.as_mut().poll(&mut cx) {
            return v;
        }
        std::thread::yield_now();
    }
}

/// 通道裡的值作為硬體 token 的整數（bool 為 0 / 1）
fn hw_int(v: &Value) -> i128 {
    match v { Value::Int(n) => *n, Value::Bool(b) => *b as i128, Value::Fixed(r, _) => *r, _ => 0 }
}

/// 硬體送出的整數放進型別為 `t` 的通道
fn hw_value(n: i128, t: &RtType) -> Value {
    match t { RtType::Bool => Value::Bool(n != 0), RtType::Fixed { f, .. } => Value::Fixed(n, *f), _ => Value::Int(n) }
}

impl Shared {
    /// 硬體任務 `task`：一個時脈一個時脈地把 KPN 通道接到它的 valid / ready 埠。
    /// 沒有握手、內部也沒變時它閒著，等任何通道有變化（和其他任務一樣參與死結偵測）；
    /// 所有輸出通道都關了又閒著時，任務結束。
    pub async fn run_hw(self: Arc<Self>, task: usize, ins: Vec<usize>, outs: Vec<usize>, mut hw: Box<dyn HwTask>) -> R<()> {
        loop {
            if self.stopped() {
                return Err(RtErr::Shutdown);
            }
            // the epoch with the snapshot: a change after it (another task sending) wakes the wait below
            let (offer, room, e0): (Vec<Option<HwTok>>, Vec<bool>, u64) = {
                let w = self.lock();
                (
                    ins.iter().map(|&c| {
                        let ch = &w.chans[c];
                        match ch.queue.front() { Some(v) => Some(HwTok::Data(hw_int(v))), None if ch.closed => Some(HwTok::Close), None => None }
                    }).collect(),
                    outs.iter().map(|&c| { let ch = &w.chans[c]; !ch.closed && ch.queue.len() < ch.depth }).collect(),
                    w.epoch,
                )
            };
            let (taken, sent, changed) = hw.cycle(&offer, &room);
            let mut moved = false;
            {
                let mut w = self.lock();
                for (k, &c) in ins.iter().enumerate() {
                    if taken.get(k).copied().unwrap_or(false) {
                        if let Some(HwTok::Data(_)) = offer[k] {
                            w.chans[c].queue.pop_front();
                            w.chans[c].received += 1;
                        }
                        moved = true;
                    }
                }
                for (k, &c) in outs.iter().enumerate() {
                    match sent.get(k).cloned().flatten() {
                        Some(HwTok::Data(n)) => {
                            let v = hw_value(n, &w.chans[c].elem);
                            if self.trace { self.trace_line(format!("[{:>6}] {:<24} send {}", w.seq, w.chans[c].name, v)); }
                            let ch = &mut w.chans[c];
                            ch.queue.push_back(v);
                            ch.sent += 1;
                            ch.max_occ = ch.max_occ.max(ch.queue.len());
                            moved = true;
                        }
                        Some(HwTok::Close) => { w.chans[c].closed = true; moved = true; }
                        None => {}
                    }
                }
                if moved { w.bump(); }
            }
            if moved {
                self.cv.notify_all();
                continue;
            }
            if changed {
                continue;
            }
            if !outs.is_empty() && outs.iter().all(|&c| self.lock().chans[c].closed) {
                return Ok(());
            }
            self.wait_until(task, &|_| "hardware (gate-level) waiting on its channels".to_string(), |w| if w.epoch != e0 { Some(Ok(())) } else { None }).await?;
        }
    }
}
