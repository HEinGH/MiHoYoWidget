import { invoke } from "@tauri-apps/api/core";
import { emit } from "@tauri-apps/api/event";

/**
 * ⚠️ v1.3.0（2026-10-14）：冻结原神便签（`genshin_note_enabled`，默认 false）；
 *   zzz 便签 + 签到可用；战绩域一律"仅手动刷新"。
 *
 * 🔴 铁律：战绩域（便签）**只能由用户手动点击「刷新」按钮触发**。
 * 禁止在任何自动路径中调用 `api.getDailyNote`：
 *   - 定时器（签到域刷新定时器）
 *   - `init()` / `refreshAll()` / 切页签 / 打开关闭设置页 / 登录成功回调
 * 理由：战绩域是**滑动窗口风控**，每次请求都重新计时。历史两次事故
 * （08-30「5 分钟自动刷新 → 1034 永续」、09-26「切页签击穿退避」）都源于
 * "自动/交互路径打战绩域"。2026-09-26 ~ 10-14 停全战绩域请求 18 天后窗口才关闭。
 */

export interface SignInfo {
  game_id: string;
  signed: boolean;
  total_days: number;
  missed: number;
}

export interface GameAccount {
  game_id: string;
  game_name: string;
  game_icon: string;
  uid: string;
  nickname: string;
  server: string;
  level: number;
}

export interface AppState {
  has_cookies: boolean;
  games: string[];
  auto_checkin: boolean;
  auto_start: boolean;
  transparency: number;
  /** 字体大小倍数（1.0=最小档） */
  font_scale: number;
  allow_manual_cookie: boolean;
  /**
   * 签到域自动刷新间隔（秒，后端已夹取 ≥900）。
   * ⚠️ 只作用于**签到域**；战绩域（便签）永远是"仅手动刷新"，不受此值影响。
   */
  refresh_interval: number;
  /**
   * 「原神便签」开关（v1.3.0 起默认 false = **冻结**）。
   * 冻结时便签卡不提供原神便签的刷新入口，且 store/后端都会拒绝请求（不发网络请求）。
   */
  genshin_note_enabled: boolean;
  /**
   * 二进制版本号（由后端 `env!("CARGO_PKG_VERSION")` 提供）。
   * ⚠️ 底部版本栏必须用它渲染，**不要硬编码**（历史 bug：v1.3.0 仍显示 v1.2.1）。
   */
  version: string;
}

/**
 * 字体大小档位（1.0=当前最小档；可选档位受窗口宽度限制）：
 * 内容宽度估算：账号信息行 ≈ 170×scale + 30px，窗口可用宽 = 视口宽 - 28
 * <280 仅1.0x / 280~340 可1.2x / 340~400 可1.4x / ≥400 全档
 * ⚠️ 窗口缩小时若当前档位超限，需自动降级（App.tsx 全局 resize 监听）
 */
export const FONT_LEVELS = [
  { scale: 1.0, label: "小", minWidth: 0 },
  { scale: 1.2, label: "标准", minWidth: 280 },
  { scale: 1.4, label: "大", minWidth: 340 },
  { scale: 1.6, label: "特大", minWidth: 400 },
];

/** 窗口宽度下允许的最大档位（用于超限自动降级） */
export function maxFontLevel(winWidth: number) {
  return [...FONT_LEVELS].reverse().find((lv) => winWidth >= lv.minWidth);
}

/* ------------------------------------------------------------------ *
 * 每日便签（战绩域）—— 配置驱动渲染
 * ⚠️ 数据**只能由用户手动点击「刷新」**获取，见文件头铁律。
 * ------------------------------------------------------------------ */

export interface NoteResin {
  current: number;
  max: number;
  recovery_time_seconds: number;
}

/** Rust 侧 `api/notes.rs` 的语义化输出（未展示的字段也一并声明，便于扩展） */
export interface DailyNote {
  game_id: string;
  game_name: string;
  game_icon: string;
  uid: string;
  nickname: string;
  resin?: NoteResin;
  vitality?: { current: number; max: number };
  commissions?: { completed: number; max: number; finished: boolean };
  expeditions?: { completed: number; max: number; ongoing: number };
  home_coin?: { current: number; max: number; is_full: boolean };
  vhs_sale?: { state: string };
  card_sign?: { state: string };
  bounty_commission?: { current: number; max: number };
  abyss_refresh?: { seconds: number };
  weekly_task?: { current: number; max: number };
  member_card?: { is_open: boolean };
  cafe_state?: { state: string };
  temple_running?: { level: number };
}

/** zzz 枚举 → 中文（未知枚举回退原始串，避免服务端加枚举时白屏） */
export const ZZZ_TEXT_MAP: Record<string, string> = {
  SaleStateDoing: "营业中",
  SaleStateDone: "已完成",
  SaleStateNotOpen: "未开业",
  CardSignDone: "已领取",
  CardSignNotDone: "未领取",
  CafeStateDone: "已领取",
  CafeStateNotOpen: "未开放",
  CafeStateDoing: "进行中",
};

/** 行渲染描述：新增字段只需在 `NOTE_CONFIG` 加一条，不必改组件 */
export interface NoteRowDef {
  key: string;
  label: string;
  type: "bar" | "vitality" | "time" | "text";
  /** 文本值；返回 "" 表示该字段在当前响应里缺失 → 该行不渲染 */
  text: (n: DailyNote) => string;
  /** 进度比例 0~1（type=bar / vitality 用） */
  ratio?: (n: DailyNote) => number;
  /** 满状态高亮 */
  full?: (n: DailyNote) => boolean;
}

/** 秒 → "3小时22分" / "已满" */
export function formatDuration(sec: number): string {
  if (!sec || sec <= 0) return "已满";
  const h = Math.floor(sec / 3600);
  const m = Math.floor((sec % 3600) / 60);
  return h > 0 ? `${h}小时${m}分` : `${m}分钟`;
}

const zzzText = (s?: { state: string }) =>
  s && s.state ? (ZZZ_TEXT_MAP[s.state] ?? s.state) : "";

const barRatio = (cur?: number, max?: number) =>
  max && max > 0 ? Math.min(1, (cur ?? 0) / max) : 0;

/** 便签行配置：`external` = 卡片常显，`expanded` = 展开后显示 */
export const NOTE_CONFIG: Record<
  string,
  { external: NoteRowDef[]; expanded: NoteRowDef[] }
> = {
  zzz: {
    external: [
      {
        key: "energy",
        label: "电量",
        type: "bar",
        text: (n) => (n.resin ? `${n.resin.current}/${n.resin.max}` : ""),
        ratio: (n) => barRatio(n.resin?.current, n.resin?.max),
        full: (n) => !!n.resin && n.resin.max > 0 && n.resin.current >= n.resin.max,
      },
      {
        key: "energy_full",
        label: "回满时间",
        type: "time",
        text: (n) => (n.resin ? formatDuration(n.resin.recovery_time_seconds) : ""),
      },
      {
        key: "vitality",
        label: "今日活跃度",
        type: "vitality",
        text: (n) => (n.vitality ? `${n.vitality.current}/${n.vitality.max}` : ""),
        ratio: (n) => barRatio(n.vitality?.current, n.vitality?.max),
      },
    ],
    expanded: [
      { key: "vhs_sale", label: "录像店经营", type: "text", text: (n) => zzzText(n.vhs_sale) },
      { key: "card_sign", label: "每日盲盒", type: "text", text: (n) => zzzText(n.card_sign) },
      { key: "cafe_state", label: "咖啡店状态", type: "text", text: (n) => zzzText(n.cafe_state) },
      {
        key: "bounty",
        label: "周常悬赏积分",
        type: "bar",
        text: (n) =>
          n.bounty_commission
            ? `${n.bounty_commission.current}/${n.bounty_commission.max}`
            : "",
        ratio: (n) => barRatio(n.bounty_commission?.current, n.bounty_commission?.max),
        full: (n) =>
          !!n.bounty_commission &&
          n.bounty_commission.max > 0 &&
          n.bounty_commission.current >= n.bounty_commission.max,
      },
      {
        key: "weekly_task",
        label: "周常活跃度",
        type: "bar",
        text: (n) => (n.weekly_task ? `${n.weekly_task.current}/${n.weekly_task.max}` : ""),
        ratio: (n) => barRatio(n.weekly_task?.current, n.weekly_task?.max),
        full: (n) =>
          !!n.weekly_task && n.weekly_task.max > 0 && n.weekly_task.current >= n.weekly_task.max,
      },
      {
        key: "abyss_refresh",
        label: "周常刷新",
        type: "time",
        text: (n) => (n.abyss_refresh ? formatDuration(n.abyss_refresh.seconds) : ""),
      },
      {
        key: "member_card",
        label: "绳网会员",
        type: "text",
        text: (n) => (n.member_card ? (n.member_card.is_open ? "已开通" : "未开通") : ""),
      },
      {
        key: "temple",
        label: "录像店等级",
        type: "text",
        text: (n) => (n.temple_running ? `Lv.${n.temple_running.level}` : ""),
      },
    ],
  },
  genshin: {
    external: [
      {
        key: "resin",
        label: "原粹树脂",
        type: "bar",
        text: (n) => (n.resin ? `${n.resin.current}/${n.resin.max}` : ""),
        ratio: (n) => barRatio(n.resin?.current, n.resin?.max),
        full: (n) => !!n.resin && n.resin.max > 0 && n.resin.current >= n.resin.max,
      },
      {
        key: "resin_full",
        label: "回满时间",
        type: "time",
        text: (n) => (n.resin ? formatDuration(n.resin.recovery_time_seconds) : ""),
      },
      {
        key: "commissions",
        label: "每日委托",
        type: "bar",
        text: (n) => (n.commissions ? `${n.commissions.completed}/${n.commissions.max}` : ""),
        ratio: (n) => barRatio(n.commissions?.completed, n.commissions?.max),
        full: (n) => !!n.commissions && n.commissions.finished,
      },
      {
        key: "expeditions",
        label: "派遣委托",
        type: "text",
        text: (n) => (n.expeditions ? `${n.expeditions.completed}/${n.expeditions.max}` : ""),
      },
    ],
    expanded: [
      {
        key: "home_coin",
        label: "洞天宝钱",
        type: "bar",
        text: (n) => (n.home_coin ? `${n.home_coin.current}/${n.home_coin.max}` : ""),
        ratio: (n) => barRatio(n.home_coin?.current, n.home_coin?.max),
        full: (n) => !!n.home_coin && n.home_coin.is_full,
      },
    ],
  },
};

export const api = {
  getAppState: () => invoke<AppState>("get_app_state"),
  getLoginStatus: () => invoke<{ has_cookies: boolean; valid: boolean }>("get_login_status"),
  /**
   * 手动粘贴 Cookie 登录。
   *
   * ⚠️ 若 cookie 来自**米游社 App 抓包**，必须一并提供抓包请求头里的
   * `x-rpc-device_id`（`deviceId`）——v2 会话与设备绑定，只搬 cookie 会被判 `-100`；
   * `deviceFp`（`x-rpc-device_fp` / `DEVICEFP`）留空时会自动沿用既有值。
   */
  loginWithCookies: (
    cookiesMap: Record<string, string>,
    deviceId?: string,
    deviceFp?: string,
  ) =>
    invoke<{ ok: boolean; note_check: string }>("login_with_cookies", {
      cookiesMap,
      deviceId: deviceId ?? "",
      deviceFp: deviceFp ?? "",
    }),
  startQrLogin: () => invoke<void>("start_qr_login"),
  logout: () => invoke<void>("logout"),
  /** 开机自启：联动系统注册（autostart 插件）+ 同步 config */
  setAutoStart: (enabled: boolean) => invoke<boolean>("set_auto_start", { enabled }),
  /** 查询系统开机自启真实状态 */
  getAutoStart: () => invoke<boolean>("get_auto_start"),
  /** 通知 Rust 侧执行窗口拖拽（规避前端限制） */
  startDrag: () => emit("start-drag"),
  /** 通知 Rust 侧隐藏窗口到托盘 */
  hideWindow: () => emit("hide-window"),
  getGames: () => invoke<{ games: GameAccount[]; logged_in: boolean }>("get_games"),
  getSignInfo: (gameId: string) => invoke<SignInfo>("get_sign_info", { gameId }),
  manualSignIn: (gameId: string) => invoke<SignInfo>("manual_sign_in", { gameId }),
  /**
   * ⚠️ 战绩域（每日便签）——**只能由用户手动点击「刷新」按钮调用**。
   * 禁止在任何自动路径中调用（定时器 / init / refreshAll / 切页签 / 设置页 / 登录回调）。
   */
  getDailyNote: (gameId: string) => invoke<DailyNote>("get_daily_note", { gameId }),
  /**
   * ⚠️ 战绩域：创建 geetest 挑战（返回 gt + challenge）。
   * 只能由便签卡上的「去验证」按钮显式触发；且**必须先点过「刷新」**
   * （便签请求返回 1034 时才会捕获到 x-trace-id，挑战需要它）。
   */
  getVerificationChallenge: (gameId: string) =>
    invoke<{ gt: string; challenge: string }>("get_verification_challenge", { gameId }),
  /** 打开滑块验证窗口（加载 mihoyo 官方域页面做宿主，借其域身份过 geetest 前置校验） */
  openCaptchaWindow: (gameId: string, gt: string, challenge: string) =>
    invoke<void>("open_captcha_window", { gameId, gt, challenge }),
  /**
   * 打开米游社**官方战绩页**（不注入任何脚本），让站点自己的 JS 走完验证流程。
   * 依据「写操作优先浏览器点击（站点自算 DS、自带风控）」的实践纪律。
   */
  openOfficialPage: () => invoke<void>("open_official_page"),
  /**
   * ⚠️ **重置设备标识**：清除 WebView2 站点数据（含 `_MHYUUID`/`DEVICEFP`）与登录凭证，
   * 随后自动重启 → **需要重新扫码登录**。
   * 用途：本应用这个"设备"被服务端标记时（原神便签恒 1034），换新设备身份。
   */
  resetDeviceIdentity: () => invoke<void>("reset_device_identity"),
  getConfig: () => invoke<Record<string, unknown>>("get_config"),
  updateConfig: (key: string, value: unknown) =>
    invoke<Record<string, unknown>>("update_config", { key, value }),
};
