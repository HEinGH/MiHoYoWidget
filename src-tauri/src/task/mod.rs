//! 定时任务：启动补签 + 每小时补签（米游社 00:00 刷新，无需精确时刻）
//!
//! ⚠️ 风控纪律（2026-09-26 立 · 2026-10-14 修订）：
//!   1. **节流已下沉到 `gateway`（请求层）**，本模块**不再自行节流**。
//!      修订原因：原先 `task::throttled()` 只覆盖本模块的 3 个调用点，而 `commands` 层
//!      与 `api.sign_in()` 的内部复查询全部裸发——实测每次启动 2 次请求间隔 113ms、
//!      单会话 2.36s 内 6 连发（≈2.5 req/s，纪律值 0.33 req/s 的 ~7.5 倍）。
//!      **节流属于"请求本身"，不属于"调用点"**；只要经 `MihoyoApi` 就自动受约束。
//!   2. **禁止随机长睡**：旧实现"每账号随机睡 0~60s"导致任务时长 N×平均 30s 且不可控，
//!      改为**游戏间**随机延迟（0~10s），总时长可控。
//!   3. 自动签到只打签到域，**绝不触碰战绩域**（api-takumi-record）——
//!      战绩域自 2026-10-14 起为「**仅手动刷新**」，任何自动路径都不得请求。
use crate::api::sign::api_from_app;
use crate::storage::config;
use rand::Rng;
use std::time::Duration;
use tauri::AppHandle;

/// 游戏间随机延迟（0~10s）：同样先取值再 await，避免 ThreadRng 跨 await 持有
async fn inter_game_delay() {
    let secs: u64 = {
        let mut rng = rand::thread_rng();
        rng.gen_range(0..10)
    };
    if secs > 0 {
        tokio::time::sleep(Duration::from_secs(secs)).await;
    }
}

/// 启动调度器：5s 后启动补签，随后每小时补签
pub fn spawn_scheduler(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        // 启动补签（等 5s 让窗口与配置就绪，避免开机自启时网络/磁盘未准备好）
        tokio::time::sleep(Duration::from_secs(5)).await;
        check_and_sign(&app).await;

        // ⚠️ tokio::time::interval 的**首次 tick 立即返回**（不等待一个周期）。
        //    旧写法 `loop { interval.tick().await; check() }` 会在间隔创建后立刻
        //    再执行一次 check_and_sign → 启动时连续补签两次（重复请求，风控不友好）。
        //    此处先 tick 一次吃掉"立即返回"的那次，之后每次 tick 才真正间隔 1 小时。
        let mut interval = tokio::time::interval(Duration::from_secs(3600));
        interval.tick().await; // 消费首次立即 tick（上面已经补签过）
        loop {
            interval.tick().await; // 此处起为真正的"距上次 3600s"
            check_and_sign(&app).await;
        }
    });
}

/// 对启用且已绑定账号的游戏执行"未签则补签"
pub async fn check_and_sign(app: &AppHandle) {
    let cfg = config::load(app);
    if !cfg.auto_checkin {
        return;
    }
    let Some(api) = api_from_app(app) else { return };
    let Ok(accounts) = api.get_game_accounts().await else { return };

    for acc in &accounts {
        if !cfg.games.contains(&acc.game_id) {
            continue;
        }
        // 游戏间随机延迟 0~10s（打散多游戏请求节奏；不含请求本身，总时长可控）
        inter_game_delay().await;

        match api.get_sign_info(&acc.game_id, &acc.uid, &acc.server).await {
            Ok(info) if !info.signed => {
                let _ = api.sign_in(&acc.game_id, &acc.uid, &acc.server).await;
            }
            _ => {}
        }
    }
}
