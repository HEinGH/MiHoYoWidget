//! 游戏注册表：新增游戏只需添加一条记录（可扩展性设计，§4.5）
//!
//! ⚠️ 2026-10-14：`notes`（每日便签，战绩域）与 `captcha`（geetest 验证码）
//!    已随风控消退恢复。二者的请求全部经 `gateway`（`Domain::Record`）节流，
//!    且**只允许由前端「刷新 / 去验证」按钮显式触发**——禁止任何自动路径调用。
//!
//! ⚠️ 2026-10-15（Stage 1）：新增 `app_login` —— 官方「米游社扫码登录」App 身份会话。
//!    只打 `passport-api.miyoushe.com` / `public-data-api.mihoyo.com`（**非战绩域**），
//!    不触碰"仅手动刷新"铁律。详见模块头注释。

pub mod app_login;
pub mod captcha;
pub mod notes;
pub mod sign;

#[derive(Debug, Clone, Copy)]
pub struct GameSpec {
    pub id: &'static str,
    pub name: &'static str,
    pub icon: &'static str,
    /// 签到域 x-rpc-signgame 值
    pub sign_game_header: &'static str,
    /// 账号绑定 game_biz（绑定接口返回）
    pub biz: &'static str,
    /// 签到活动 act_id（event/luna 体系）
    pub act_id: &'static str,
    /// game_record 域 API 基础路径（原神战绩域 / zzz event 域，便签与验证码 challenge_path 用）
    pub record_base: &'static str,
}

pub const REGISTRY: &[GameSpec] = &[
    GameSpec {
        id: "genshin",
        name: "原神",
        icon: "🍃",
        sign_game_header: "hk4e",
        biz: "hk4e_cn",
        act_id: "e202311201442471",
        record_base: "/game_record/app/genshin/api",
    },
    GameSpec {
        id: "zzz",
        name: "绝区零",
        icon: "⚡",
        sign_game_header: "zzz",
        biz: "nap_cn",
        act_id: "e202406242138391",
        record_base: "/event/game_record_zzz/api/zzz",
    },
    // 未来扩展（崩铁/崩3 只需追加）：
    // GameSpec { id: "hkrpg", name: "崩坏：星穹铁道", icon: "🚂", sign_game_header: "hkrpg", biz: "hkrpg_cn", act_id: "e202304121516551", record_base: "/game_record/app/hkrpg/api" },
    // GameSpec { id: "honkai", name: "崩坏3", icon: "💥", sign_game_header: "bh3", biz: "bh3_cn", act_id: "e202306201626331", record_base: "/game_record/app/honkai3rd/api" },
];

pub fn find_spec(game_id: &str) -> Option<&'static GameSpec> {
    REGISTRY.iter().find(|g| g.id == game_id)
}

/// 根据绑定接口的 game_biz 反查注册表
pub fn find_by_biz(game_biz: &str) -> Option<&'static GameSpec> {
    REGISTRY.iter().find(|g| g.biz == game_biz)
}
