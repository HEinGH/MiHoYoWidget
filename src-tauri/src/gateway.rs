//! 统一请求网关 —— **所有米游社请求的唯一出口**
//!
//! ⚠️ 2026-10-14 新增（由工程 Review 的运行态实测驱动）
//!
//! 背景：`task::throttled()` 原先只存在于调度器的 3 个调用点，而
//!   - `commands::get_sign_info` / `manual_sign_in` / `get_games` / `get_login_status`
//!   - **`api.sign_in()` 内部的 `get_sign_info()` 复查询**
//! 全部绕过节流 → 实测每次启动都出现 2 次请求间隔 **113ms**，
//! 单次会话出现 **2.36 秒内 6 连发**（≈2.5 req/s，纪律值 0.33 req/s 的 ~7.5 倍）。
//!
//! **设计原则：节流属于"请求本身"，不属于"调用点"。**
//! 只要请求经 `MihoyoApi` 发出，就自动获得节流保护，不依赖调用者自觉——
//! 这样新增接口时不会漏，恢复战绩域功能后也不会重演"高频打便签 → 风控永续"。
//!
//! **机制：原子占槽（atomic slot reservation）**
//! 在锁内一次性算出本次请求的"许可时刻"，并把下一可用槽位推进到
//! `许可时刻 + 域间隔 + 抖动`。因此并发调用者被**串行化**为至少相隔一个间隔，
//! 而不是各自算一遍间隔再同时发出去（旧实现正是后者，这是 113ms 双发的直接成因）。
//!
//! ⚠️ 域分级（风控暴露面）：`SignIn`（签到域，无风控、仅限流）< `Record`（战绩域，滑动窗口风控）。
//! 战绩域的**真正防线是"仅手动触发"**（见前端 NotesCard），本网关只是传输层的安全联锁。

use std::sync::Mutex;
use std::time::{Duration, Instant};

/// 数据域（决定节流参数与风控语义）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Domain {
    /// 签到域 `api-takumi.mihoyo.com`：账号列表 / 签到信息 / 签到 / 换取 stoken。
    /// 无滑动窗口风控，仅有 `-500004 请求过于频繁` 限流（实测）。
    SignIn,
    /// 战绩域 `api-takumi-record.mihoyo.com`：每日便签 / 验证码 create·verify。
    /// **滑动窗口风控——每次请求重新计时，禁止自动轮询。**
    Record,
}

impl Domain {
    fn idx(self) -> usize {
        match self {
            Domain::SignIn => 0,
            Domain::Record => 1,
        }
    }

    /// 同域两次请求的最小间隔
    pub fn min_gap(self) -> Duration {
        match self {
            // 签到域限流阈值约 1~2s，取 3s 安全余量
            Domain::SignIn => Duration::from_millis(3_000),
            // 战绩域：更保守（5s）；真正防线是"仅手动触发"，此处只防突发
            Domain::Record => Duration::from_millis(5_000),
        }
    }

    fn max_jitter_ms(self) -> u64 {
        match self {
            Domain::SignIn => 1_000,
            Domain::Record => 3_000,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Domain::SignIn => "signin",
            Domain::Record => "record",
        }
    }
}

struct Gate {
    /// 下一个可用时刻（全局单一队列：跨域也串行化，更保守）
    next_slot: Option<Instant>,
}

static GATE: Mutex<Gate> = Mutex::new(Gate { next_slot: None });

/// 每域的请求计数（total / ok / fail）——用于日志内直接产出风控指标，
/// 不必再依赖外部探针推断。
static STATS: Mutex<[(u64, u64, u64); 2]> = Mutex::new([(0, 0, 0), (0, 0, 0)]);

/// 占槽：返回需等待的时长，并把该域下一可用槽位推进到 `许可时刻 + 间隔 + 抖动`。
///
/// ⚠️ 必须在**无 await 的同步块**内完成占槽并释放锁：
/// - 锁不得跨 await 持有（否则 `MutexGuard` 非 Send，async 块编译失败）；
/// - `rand::random::<u64>()` 不产生跨 await 的绑定，是安全的抖动来源
///   （对比：`let mut rng = rand::thread_rng()` 会让整个 async 块变 !Send，历史踩坑）。
pub async fn acquire(domain: Domain) {
    let wait = {
        let mut g = GATE.lock().unwrap();
        let now = Instant::now();
        // 槽位已被更早的请求推进到未来 → 本请求排在其后
        let start = match g.next_slot {
            Some(t) if t > now => t,
            _ => now,
        };
        let jitter = Duration::from_millis(rand::random::<u64>() % domain.max_jitter_ms().max(1));
        g.next_slot = Some(start + domain.min_gap() + jitter);
        start.saturating_duration_since(now)
    };
    if !wait.is_zero() {
        tokio::time::sleep(wait).await;
    }
}

/// 记录一次请求结果（在响应拿到后调用）——供离线统计风控指标
pub fn note_response(domain: Domain, endpoint: &str, retcode: Option<i64>) {
    let (total, ok, fail) = {
        let mut s = STATS.lock().unwrap();
        let slot = &mut s[domain.idx()];
        slot.0 += 1;
        match retcode {
            Some(0) => slot.1 += 1,
            Some(_) => slot.2 += 1,
            None => {}
        }
        *slot
    };
    let tag = match retcode {
        Some(0) => "ok".to_string(),
        Some(rc) => format!("rc={rc}"),
        None => "err".to_string(),
    };
    crate::debug_log::log(&format!(
        "[gw] {} {tag} {endpoint} | 累计 total={total} ok={ok} fail={fail}",
        domain.name()
    ));
}
