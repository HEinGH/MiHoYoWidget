import { create } from "zustand";
import { api, type AppState, type DailyNote, type GameAccount, type SignInfo } from "./api";

/**
 * 登录失效分类器（与 Rust ApiError::is_session_expired 判据一致）：
 * - `msg === "未登录"`：load_api 无 cookie（文件缺失/解密失败）
 * - `msg.includes("登录态不足")`：服务端拒绝登录态（retcode -100/-101 统一错误串）
 * ⚠️ 2026-08-31 修复：此前还匹配"重新扫码登录"，而设备风控(10041)的错误串恰好包含该
 *    关键词 → 单游戏级错误被误判为整个会话过期 → 主页面"未登录"而设置页"已登录"。
 *    会话死亡类错误串已在 Rust 侧全部标准化为"登录态不足"前缀，此判据已精确无歧义。
 */
export function isSessionExpiredMsg(msg: string): boolean {
  return msg === "未登录" || msg.includes("登录态不足");
}

/** init() 网络暂时性失败后的自动重试定时器（开机自启时网络可能未就绪，避免"未登录"误判） */
let initRetryTimer: ReturnType<typeof setTimeout> | undefined;

interface WidgetStore {
  state: AppState | null;
  accounts: GameAccount[];
  signInfo: Record<string, SignInfo | null>;
  /**
   * 每日便签（战绩域）。⚠️ **只由 `refreshNote` 写入，而 `refreshNote` 只由
   * 用户手动点击「刷新」按钮触发**——禁止在任何自动路径中调用（见 api.ts 头部铁律）。
   */
  notes: Record<string, DailyNote | undefined>;
  /** 便签请求失败原因（如实展示，不做自动重试） */
  noteError: Record<string, string>;
  /** 最近一次**成功**获取便签的时间戳（用于显示更新时间 + 防连点冷却） */
  noteUpdatedAt: Record<string, number>;
  /** 便签请求进行中（防连点：进行中时按钮置灰） */
  noteLoading: Record<string, boolean>;
  /** geetest 滑块验证进行中（「去验证」按钮置灰） */
  captchaBusy: Record<string, boolean>;
  /** 滑块验证结果提示（成功/失败），展示在便签卡上 */
  captchaHint: Record<string, string>;
  currentGame: string;
  loading: boolean;
  error: string;
  loginModalOpen: boolean;
  settingsOpen: boolean;
  /** 透明度实时预览值（未确认不持久化） */
  previewOpacity: number | null;

  init: () => Promise<void>;
  refresh: (gameId: string) => Promise<void>;
  refreshAll: () => Promise<void>;
  /** ⚠️ 战绩域：**仅手动刷新**。只允许由「刷新」按钮调用。 */
  refreshNote: (gameId: string) => Promise<void>;
  /**
   * geetest 滑块验证（⚠️ 战绩域，只允许由「去验证」按钮显式触发）。
   * 流程：createVerification（战绩域 1 次）→ 打开官方域滑块窗口。
   * 验证结果由 `captcha-success` / `captcha-failed` 事件回填到 `captchaHint`。
   * **成功与否都不会自动请求便签**（铁律：仅手动刷新）。
   */
  startCaptchaVerify: (gameId: string) => Promise<void>;
  setCaptchaHint: (gameId: string, hint: string) => void;
  setCurrentGame: (id: string) => void;
  setLoginModal: (open: boolean) => void;
  setSettings: (open: boolean) => void;
  setError: (msg: string) => void;
  clearError: () => void;
  setPreviewOpacity: (v: number | null) => void;
  /** 清空登录会话数据（退出登录/登录失效时调用，防止主页面残留旧账号信息） */
  clearSession: () => void;
  /** 登录失效统一处理：清空会话 + 提示"登录已过期"并弹登录窗 */
  handleLoginExpired: () => void;
}

export const useStore = create<WidgetStore>((set, get) => ({
  state: null,
  accounts: [],
  signInfo: {},
  notes: {},
  noteError: {},
  noteUpdatedAt: {},
  noteLoading: {},
  captchaBusy: {},
  captchaHint: {},
  currentGame: "",
  loading: false,
  error: "",
  loginModalOpen: false,
  settingsOpen: false,
  previewOpacity: null,

  init: async () => {
    // 每次 init 取代旧的重试计划（避免网络失败后的定时重试与用户主动操作叠加）
    if (initRetryTimer) {
      clearTimeout(initRetryTimer);
      initRetryTimer = undefined;
    }
    try {
      const state = await api.getAppState();
      set({ state });
      if (state.has_cookies) {
        try {
          const { games, logged_in } = await api.getGames();
          if (logged_in) {
            const prev = get().currentGame;
            // 保留进入设置前的页签（不切回默认第一个）；原页签不存在（如被关闭）才回退
            const keep =
              prev && games.some((g) => g.game_id === prev) ? prev : (games[0]?.game_id ?? "");
            set({ accounts: games, currentGame: keep });
            await get().refreshAll();
          } else {
            // 真"未登录/已过期"（无 cookie 文件 / cookie 损坏 / 服务端拒绝登录态）：
            // 先清空会话数据再弹登录窗——否则取消登录窗后主页面仍渲染残留的旧账号信息
            get().clearSession();
            set({ error: "登录已过期，请重新登录", loginModalOpen: true });
          }
        } catch (e) {
          const msg = String(e);
          if (isSessionExpiredMsg(msg)) {
            // 会话失效类错误（如服务端返回 -100）→ 走"登录已过期"流程
            get().handleLoginExpired();
          } else {
            // ⚠️ 网络/暂时性失败（开机自启时网络未就绪、接口超时等）：
            //    保留登录态（不清会话、不弹登录窗、不置 has_cookies=false），
            //    仅提示错误并延迟自动重试——此前 get_games 任何失败都返回 logged_in:false，
            //    导致主页面错显示"未登录"而设置页显示"已登录"（状态不一致的根因之一）。
            set({ error: msg });
            if (initRetryTimer) clearTimeout(initRetryTimer);
            initRetryTimer = setTimeout(() => void get().init(), 10_000);
          }
        }
      } else {
        // ⚠️ 未登录：同上，先清空旧账号数据再弹登录窗
        get().clearSession();
        set({ loginModalOpen: true });
      }
    } catch (e) {
      set({ error: String(e) });
    }
  },

  /**
   * 刷新当前游戏的签到信息。
   * ⚠️ v1.2.0-lite：不再请求战绩域（便签），仅调签到域接口。
   *    失败保留旧值（避免已签到数据被 null 覆盖而误显示"未签到"——2026-09-01 踩坑）。
   */
  refresh: async (gameId: string) => {
    try {
      const sign = await api
        .getSignInfo(gameId)
        .catch(() => get().signInfo[gameId] ?? null);
      set((s) => ({ signInfo: { ...s.signInfo, [gameId]: sign }, error: "" }));
    } catch (e) {
      set({ error: String(e) });
    }
  },

  refreshAll: async () => {
    const { accounts } = get();
    set({ loading: true });
    for (const acc of accounts) {
      await get().refresh(acc.game_id);
    }
    set({ loading: false });
  },

  /**
   * ⚠️ 战绩域（每日便签）——**仅手动刷新**。
   *
   * 这是全应用**唯一**会请求战绩域的入口，且只允许由 NotesCard 的「刷新」按钮调用。
   * 违反此约定 = 重演历史事故（08-30「自动刷新 → 1034 永续」/ 09-26「切页签击穿退避」）：
   * 战绩域是滑动窗口风控，每次请求都重新计时，只要存在自动请求，窗口永不关闭。
   *
   * 失败时**如实把原因写入 `noteError` 展示给用户**，不做任何自动重试
   * （旧版的 30 分钟自动退避重试已按铁律移除——它本质仍是"自动请求"）。
   */
  refreshNote: async (gameId) => {
    // ⚠️ v1.3.0「原神便签冻结」：前端第二道门 —— 冻结时直接返回，连 invoke 都不发
    //    （第一道门是 NotesCard 不渲染刷新入口；第三道门是 Rust `get_daily_note` 硬拒绝）
    if (gameId === "genshin" && !(get().state?.genshin_note_enabled ?? false)) {
      set((s) => ({
        noteError: {
          ...s.noteError,
          [gameId]:
            "原神便签已冻结（v1.3.0）：米游社现已要求 App 身份会话才能读取，本应用为网页身份，请求必被挡下。可在设置中开启「原神便签」解除冻结。",
        },
      }));
      return;
    }
    if (get().noteLoading[gameId]) return; // 防连点
    set((s) => ({ noteLoading: { ...s.noteLoading, [gameId]: true } }));
    try {
      const note = await api.getDailyNote(gameId);
      set((s) => ({
        notes: { ...s.notes, [gameId]: note },
        noteError: { ...s.noteError, [gameId]: "" },
        noteUpdatedAt: { ...s.noteUpdatedAt, [gameId]: Date.now() },
      }));
    } catch (e) {
      set((s) => ({ noteError: { ...s.noteError, [gameId]: String(e) } }));
    } finally {
      set((s) => ({ noteLoading: { ...s.noteLoading, [gameId]: false } }));
    }
  },

  /**
   * geetest 滑块验证（⚠️ 战绩域，仅可由「去验证」按钮触发）。
   *
   * 前置：必须先点过「刷新」——便签返回 1034 时 Rust 侧才捕获到 `x-trace-id`，
   * 而挑战创建依赖它；缺失时后端会直接报错（**不会替用户补发便签请求**，遵守铁律）。
   *
   * 本动作只做两件事：① createVerification 拿 gt/challenge（战绩域 1 次）
   * ② 打开官方域滑块窗口。**成功/失败都不自动请求便签**，由用户再点「刷新」。
   */
  startCaptchaVerify: async (gameId) => {
    if (get().captchaBusy[gameId]) return;
    set((s) => ({
      captchaBusy: { ...s.captchaBusy, [gameId]: true },
      captchaHint: { ...s.captchaHint, [gameId]: "" },
    }));
    try {
      const { gt, challenge } = await api.getVerificationChallenge(gameId);
      await api.openCaptchaWindow(gameId, gt, challenge);
      set((s) => ({
        captchaHint: {
          ...s.captchaHint,
          [gameId]: "已打开验证窗口，请在其中完成滑块验证",
        },
      }));
    } catch (e) {
      set((s) => ({
        captchaHint: { ...s.captchaHint, [gameId]: `无法发起验证：${String(e)}` },
      }));
    } finally {
      set((s) => ({ captchaBusy: { ...s.captchaBusy, [gameId]: false } }));
    }
  },

  setCurrentGame: (id) => set({ currentGame: id }),
  setLoginModal: (open) => set({ loginModalOpen: open }),
  setSettings: (open) => set({ settingsOpen: open }),
  setError: (msg) => set({ error: msg }),
  clearError: () => set({ error: "" }),
  setPreviewOpacity: (v) => set({ previewOpacity: v }),
  setCaptchaHint: (gameId, hint) =>
    set((s) => ({ captchaHint: { ...s.captchaHint, [gameId]: hint } })),
  clearSession: () =>
    set((s) => ({
      accounts: [],
      signInfo: {},
      // ⚠️ 便签数据也必须清空：否则退出登录/登录失效后主页面仍渲染旧账号的体力数据
      notes: {},
      noteError: {},
      noteUpdatedAt: {},
      noteLoading: {},
      captchaBusy: {},
      captchaHint: {},
      currentGame: "",
      error: "",
      // 同步登录标记：退出后主页面立即显示"未登录"，避免 init 完成前短暂残留旧登录态
      state: s.state ? { ...s.state, has_cookies: false } : s.state,
    })),
  handleLoginExpired: () => {
    get().clearSession();
    set({ error: "登录已过期，请重新登录", loginModalOpen: true });
  },
}));
