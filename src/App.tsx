import { useCallback, useEffect, useState, type CSSProperties } from "react";
import { listen } from "@tauri-apps/api/event";
import {
  api,
  maxFontLevel,
  NOTE_CONFIG,
  type DailyNote,
  type NoteRowDef,
} from "./api";
import { LoginModal } from "./components/LoginModal";
import { SettingsModal } from "./components/SettingsModal";
import { useStore } from "./store";

/** 签到卡 */
function SignCard({ gameId }: { gameId: string }) {
  const { signInfo, refresh, setError } = useStore();
  const checkin = signInfo[gameId];
  const [signing, setSigning] = useState(false);

  const doSign = async () => {
    if (signing) return;
    setSigning(true);
    try {
      await api.manualSignIn(gameId);
      await refresh(gameId);
    } catch (e) {
      setError(String(e));
    } finally {
      setSigning(false);
    }
  };

  // ⚠️ 签到状态三态（2026-09-01 修复）：signInfo 为 undefined（未加载）/ null（获取失败）
  //    时严禁显示"今日未签到"——曾致"显示未签到但实际已签到"（数据未就绪误判）。
  const signState =
    checkin === undefined
      ? "loading"
      : checkin === null
        ? "error"
        : checkin.signed
          ? "signed"
          : "unsigned";

  return (
    <div className="card">
      <div className="card-title">
        <span>签到</span>
        <span style={{ color: "var(--text-faint)" }}>
          {checkin ? `本月 ${checkin.total_days} 天` : "—"}
        </span>
      </div>
      <div className="sign-status">
        {signState === "signed" ? (
          <>
            <span className="icon">✅</span>
            <span>今日已签到</span>
          </>
        ) : signState === "loading" ? (
          <>
            <span className="icon">⏳</span>
            <span>签到状态获取中...</span>
          </>
        ) : signState === "error" ? (
          <>
            <span className="icon">⚠️</span>
            <span>签到状态获取失败</span>
          </>
        ) : (
          <>
            <span className="icon">⚠️</span>
            <span>今日未签到</span>
          </>
        )}
      </div>
      <div className="month-progress">
        <div
          className="fill"
          style={{ width: `${Math.min(100, ((checkin?.total_days ?? 0) / 31) * 100)}%` }}
        />
      </div>
      <div className="month-progress-label">
        {checkin ? `本月已签到 ${checkin.total_days} 天` : "等待数据..."}
      </div>
      {signState === "unsigned" && (
        <button className="sign-btn" onClick={doSign} disabled={signing}>
          {signing ? "签到中..." : "点击签到"}
        </button>
      )}
    </div>
  );
}

/**
 * 便签单行（配置驱动：由 `NOTE_CONFIG` 描述，本组件不含任何游戏特有字段名）
 * - `bar` / `vitality`：中间显示进度条，右侧显示 `current/max`
 * - `time` / `text`：文字横跨中右两列
 */
function NoteRow({ row, note }: { row: NoteRowDef; note: DailyNote }) {
  const text = row.text(note);
  if (!text) return null; // 字段在当前响应里缺失 → 该行不渲染
  const isBar = row.type === "bar" || row.type === "vitality";
  const full = row.full ? row.full(note) : false;
  return (
    <div className={`note-row${full ? " full" : ""}`}>
      <span className="note-label">{row.label}</span>
      {isBar ? (
        <>
          <span className={`note-bar ${row.type}`}>
            <span
              className="note-bar-fill"
              style={{ width: `${Math.round((row.ratio ? row.ratio(note) : 0) * 100)}%` }}
            />
          </span>
          <span className="note-value">{text}</span>
        </>
      ) : (
        <span className="note-value note-span">{text}</span>
      )}
    </div>
  );
}

/** 成功后冷却（防连点，避免用户连续点击变成"事实上的高频请求"） */
const NOTE_COOLDOWN_MS = 60_000;
/** 失败后冷却（较短，允许用户修正问题后立即重试） */
const NOTE_RETRY_MS = 5_000;

/** 把 Rust 返回的便签错误串翻译为「原因 + 建议」 */
function describeNoteError(raw: string): { text: string; hint?: string } {
  if (raw.includes("CAPTCHA_REQUIRED")) {
    return {
      text: "该游戏便签被服务端验证码保护（1034）",
      hint: "建议：打开米游社 App → 我的 → 我的角色 → 完成验证码，再回来点「刷新」。签到功能不受影响。",
    };
  }
  if (raw.includes("10041")) {
    return {
      text: "该游戏便签暂不可用（服务端设备风控 10041）",
      hint: "不影响签到。稍后再点「刷新」重试即可——本应用不会自动重试。",
    };
  }
  return { text: raw.replace(/^CAPTCHA_REQUIRED\|/, "") };
}

/**
 * 便签卡（战绩域）
 *
 * 🔴 铁律：数据**只能由用户点击本卡片的「刷新」按钮获取**。
 * 本组件不做任何自动请求（不 useEffect 预取、不随 gameId 变化自动拉取）。
 * 理由：战绩域是滑动窗口风控，每次请求都重新计时；只要存在自动请求，窗口永不关闭。
 */
function NotesCard({ gameId, onRefreshSign }: { gameId: string; onRefreshSign: () => Promise<void> }) {
  const {
    state,
    notes,
    noteError,
    noteUpdatedAt,
    noteLoading,
    refreshNote,
    captchaBusy,
    captchaHint,
    startCaptchaVerify,
  } = useStore();
  const note = notes[gameId];
  // ⚠️ v1.3.0「原神便签冻结」（默认开启）：米游社现已要求 App 身份会话才能读取原神 dailyNote，
  //    本应用为网页身份，请求恒被挡下且验证无法通过 —— 冻结后本卡片**不提供**原神便签刷新入口。
  const genshinFrozen = gameId === "genshin" && !(state?.genshin_note_enabled ?? false);
  const rawErr = noteError[gameId] ?? "";
  const busy = !!noteLoading[gameId];
  const [expanded, setExpanded] = useState(false);
  const [now, setNow] = useState(() => Date.now());

  // 切游戏时收起展开区（避免上一个游戏的展开状态串到下一个）
  useEffect(() => {
    setExpanded(false);
  }, [gameId]);

  // 冷却倒计时：仅在存在冷却时才有意义，1s tick 成本可忽略
  useEffect(() => {
    const t = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(t);
  }, []);

  const last = noteUpdatedAt[gameId] ?? 0;
  const cooldownMs = note ? NOTE_COOLDOWN_MS : NOTE_RETRY_MS;
  const remainMs = last > 0 ? Math.max(0, cooldownMs - (now - last)) : 0;
  const cooling = remainMs > 0;

  const doRefresh = async () => {
    // 便签（战绩域，受铁律约束）+ 签到（安全域）一起刷新，保持卡片数据一致
    // ⚠️ 原神便签冻结时**不发**便签请求，只刷新签到（签到域无风控）
    if (genshinFrozen) {
      await onRefreshSign();
      return;
    }
    await Promise.all([refreshNote(gameId), onRefreshSign()]);
  };

  const cfg = NOTE_CONFIG[gameId];
  const rowsOf = (defs: NoteRowDef[] | undefined) =>
    note && defs ? defs.filter((r) => r.text(note) !== "") : [];
  const externalRows = rowsOf(cfg?.external);
  const expandedRows = rowsOf(cfg?.expanded);
  const err = rawErr ? describeNoteError(rawErr) : null;
  // 1034 = 验证码保护：仅此时才提供「去验证」（滑块链路，战绩域请求，全手动）
  const needCaptcha = rawErr.includes("CAPTCHA_REQUIRED");

  return (
    <div className="card">
      <div className="card-title">
        <span>便签</span>
        <span className="note-actions">
          {last > 0 && (
            <span className="note-updated">
              {new Date(last).toLocaleTimeString("zh-CN", {
                hour: "2-digit",
                minute: "2-digit",
              })}
            </span>
          )}
          <button
            className="note-refresh-btn"
            disabled={busy || cooling}
            title={
              genshinFrozen
                ? "原神便签已冻结：此处只刷新签到（签到域无风控）"
                : "战绩域仅手动刷新：本应用不会自动请求便签"
            }
            onClick={() => void doRefresh()}
          >
            {busy
              ? "刷新中"
              : cooling
                ? `${Math.ceil(remainMs / 1000)}s`
                : genshinFrozen
                  ? "⟳ 刷新签到"
                  : "⟳ 刷新"}
          </button>
        </span>
      </div>

      {captchaHint[gameId] && (
        <div className="note-captcha-hint">{captchaHint[gameId]}</div>
      )}

      {genshinFrozen ? (
        <div className="note-placeholder">
          原神便签已冻结（v1.3.0）
          <span className="note-placeholder-sub">
            米游社现已要求「App 身份」会话才能读取原神便签；本应用为网页身份，
            请求必被 1034 挡下且验证无法通过（已实测），故默认冻结以免白白暴露风控面。
            可在设置中开启「原神便签」解除冻结（建议先导入 App 形式会话）。
          </span>
        </div>
      ) : err ? (
        <div className="note-error">
          <div className="note-error-text">⚠️ {err.text}</div>
          {err.hint && <div className="note-error-hint">{err.hint}</div>}
          {needCaptcha && (
            <button
              className="note-verify-btn"
              disabled={!!captchaBusy[gameId]}
              onClick={() => void startCaptchaVerify(gameId)}
            >
              {captchaBusy[gameId] ? "正在打开验证窗口…" : "去验证（滑块）"}
            </button>
          )}
        </div>
      ) : !note ? (
        <div className="note-placeholder">
          点击「刷新」获取便签数据
          <span className="note-placeholder-sub">（战绩域仅手动刷新，不会自动请求）</span>
        </div>
      ) : externalRows.length === 0 && expandedRows.length === 0 ? (
        <div className="note-placeholder">该游戏便签无可展示字段</div>
      ) : (
        <>
          <div className="notes-grid">
            {externalRows.map((row) => (
              <NoteRow key={row.key} row={row} note={note} />
            ))}
          </div>
          {expandedRows.length > 0 && (
            <>
              <button className="expand-btn" onClick={() => setExpanded((v) => !v)}>
                {expanded ? "收起 ▲" : "展开更多 ▼"}
              </button>
              {expanded && (
                <div className="notes-grid">
                  {expandedRows.map((row) => (
                    <NoteRow key={row.key} row={row} note={note} />
                  ))}
                </div>
              )}
            </>
          )}
        </>
      )}
    </div>
  );
}

export default function App() {
  const {
    state,
    accounts,
    currentGame,
    loading,
    error,
    loginModalOpen,
    settingsOpen,
    previewOpacity,
    init,
    refresh,
    setCurrentGame,
    setSettings,
  } = useStore();

  // 昵称/UID 从账号列表取（不依赖任何战绩域数据）
  const currentAccount = accounts.find((a) => a.game_id === currentGame);

  useEffect(() => {
    void init();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  /**
   * 签到域自动刷新定时器（间隔由 config.refresh_interval 决定，后端已夹取 ≥15 分钟）。
   * ⚠️ **只打签到域**——绝不含任何战绩域（便签）请求。便签永远是"仅手动刷新"。
   */
  const refreshMs = (state?.refresh_interval ?? 900) * 1000;
  useEffect(() => {
    const timer = window.setInterval(() => {
      const id = useStore.getState().currentGame;
      if (!id) return;
      void useStore.getState().refresh(id);
    }, refreshMs);
    return () => window.clearInterval(timer);
  }, [refreshMs]);

  // 字体大小：根字号 = 基准 12px × 档位倍数（CSS 全部用 rem，随 html font-size 缩放）
  useEffect(() => {
    const scale = state?.font_scale ?? 1.0;
    document.documentElement.style.fontSize = `${12 * scale}px`;
  }, [state?.font_scale]);

  // ⚠️ 窗口缩小时自动降级字体档位（2026-08-30 修复）：
  //   用户先放大窗口选大字号 → 再缩小窗口 → 档位超限必须自动降级，
  //   否则页面保持大字号溢出（设置页内按钮虽置灰但页面不变）。
  //   全局监听：设置页开/关都生效；降级持久化 + 立即应用。
  useEffect(() => {
    const onResize = () => {
      const w = window.innerWidth;
      const cur = useStore.getState().state?.font_scale ?? 1.0;
      const maxAllowed = maxFontLevel(w);
      if (maxAllowed && cur > maxAllowed.scale) {
        document.documentElement.style.fontSize = `${12 * maxAllowed.scale}px`;
        useStore.setState((s) =>
          s.state ? { state: { ...s.state, font_scale: maxAllowed.scale } } : {},
        );
        void api.updateConfig("font_scale", maxAllowed.scale);
      }
    };
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, []);

  // 登录状态周期探测（每 10 分钟）：cookie 在后台过期时自动切换为"登录已过期"。
  //   getLoginStatus 内部真实调账号接口判定，三态分流（网络失败不判失效）。
  useEffect(() => {
    const t = window.setInterval(() => {
      void api
        .getLoginStatus()
        .then((s) => {
          if (s.has_cookies && !s.valid) useStore.getState().handleLoginExpired();
        })
        .catch(() => {});
    }, 600_000);
    return () => window.clearInterval(t);
  }, []);

  // 托盘「立即签到」完成事件：此前 Rust 侧 emit 了但前端无人监听 →
  //   用户从托盘点签到后界面不刷新（要等定时器或手动切页签）。此处补齐。
  useEffect(() => {
    const unlisten = listen("checkin-done", () => {
      void useStore.getState().refreshAll();
    });
    return () => {
      void unlisten.then((fn) => fn());
    };
  }, []);

  // geetest 滑块验证结果（Rust 侧 emit，事件名刻意与 captcha 窗口→Rust 的四事件区分开，防回环）
  //   ⚠️ 验证成功后**只提示用户点「刷新」**，绝不自动请求便签（铁律：仅手动刷新）
  useEffect(() => {
    const okP = listen<{ game_id: string }>("captcha-success", (e) => {
      const gid = e.payload?.game_id ?? "";
      if (!gid) return;
      const st = useStore.getState();
      // 清掉 1034 报错，让卡片回到"点击刷新"的可操作状态
      useStore.setState((s) => ({ noteError: { ...s.noteError, [gid]: "" } }));
      st.setCaptchaHint(gid, "✅ 验证已完成，请点「刷新」获取便签");
    });
    const failP = listen<{ game_id?: string; message?: string }>("captcha-failed", (e) => {
      const st = useStore.getState();
      const gid = e.payload?.game_id || st.currentGame;
      if (!gid) return;
      st.setCaptchaHint(gid, `❌ 验证失败：${e.payload?.message ?? "未知原因"}`);
    });
    return () => {
      void okP.then((fn) => fn());
      void failP.then((fn) => fn());
    };
  }, []);

  const switchGame = useCallback(
    (id: string) => {
      setCurrentGame(id);
      // ⚠️ 只刷新签到域（无风控）。切页签**绝不**触发便签请求——
      //    09-26 事故即"切页签 force 刷新便签"持续击穿退避、延长风控窗口。
      void refresh(id);
    },
    [refresh, setCurrentGame],
  );

  // 不透明度联动值（全部内联到 style，绕开 CSS 变量嵌套 var() 在 WebView2 的解析问题）
  const opacity = previewOpacity ?? state?.transparency ?? 0.9;
  const bgAlpha = Math.max(0.5, opacity);
  const bgBlur = opacity >= 0.98 ? "none" : "blur(18px)";
  // 圆角固定 12px（内联样式解决透明问题后，不再需要随透明度变化圆角）
  const shellRadius = "12px";

  return (
    <div
      className="widget-shell"
      style={{
        opacity,
        // ⚠️ 内联完整 rgba（不依赖 --bg-alpha 变量嵌套）：
        //    WebView2 对 rgba(..., var(--x)) 解析可能回退到 0.88，导致主页面永远 12% 透
        background: `rgba(24, 24, 38, ${bgAlpha})`,
        // ⚠️ 完全不透明时必须为 none 彻底移除 backdrop-filter（blur(0) 仍激活透明渲染路径）
        backdropFilter: bgBlur,
        WebkitBackdropFilter: bgBlur,
        // 完全实色时圆角归零，背景铺满视口（杜绝四角透出）
        borderRadius: shellRadius,
      } as CSSProperties}
    >
      {/* 顶部：可拖拽区域（Rust 侧执行拖拽，规避前端限制） */}
      <div
        className="widget-header"
        onMouseDown={(e) => {
          if (e.button === 0 && e.target === e.currentTarget) {
            void api.startDrag();
          }
        }}
      >
        <div className="game-tabs">
          {accounts.map((acc) => (
            <button
              key={acc.game_id}
              className={`game-tab ${currentGame === acc.game_id ? "active" : ""}`}
              onClick={() => switchGame(acc.game_id)}
            >
              {acc.game_icon} {acc.game_name}
            </button>
          ))}
        </div>
        <div className="header-actions">
          <button className="icon-btn" title="设置" onClick={() => setSettings(true)}>
            ⚙
          </button>
          <button className="icon-btn" title="隐藏到托盘" onClick={() => void api.hideWindow()}>
            ✕
          </button>
        </div>
      </div>

      {/* 主体 */}
      <div className="widget-body">
        {/*
          ⚠️ 错误横幅置于最顶层（2026-08-31 修复）：此前仅在"已登录且有账号"分支内渲染，
          accounts 为空（网络失败/尚未拉取）时错误不可见，用户只看到误导性的
          "未绑定游戏账号"（实际是网络/接口暂时失败）。现在任何状态下错误都可见。
        */}
        {error && <div className="error-banner">{error}</div>}
        {state === null ? (
          <div style={{ textAlign: "center", padding: 40, color: "var(--text-dim)", fontSize: 13 }}>
            加载中...
          </div>
        ) : !state.has_cookies ? (
          <div style={{ textAlign: "center", padding: 40, color: "var(--text-dim)", fontSize: 13 }}>
            未登录，点击右上角设置登录
          </div>
        ) : accounts.length === 0 ? (
          <div style={{ textAlign: "center", padding: 40, color: "var(--text-dim)", fontSize: 13 }}>
            {loading ? "加载中..." : "未绑定游戏账号"}
          </div>
        ) : (
          <>
            <div className="game-meta">
              <div className="title">{currentAccount?.nickname || ""}</div>
              <div className="sub">
                {currentAccount?.uid ? `UID ${currentAccount.uid}` : ""}
              </div>
            </div>
            <SignCard gameId={currentGame} />
            <NotesCard gameId={currentGame} onRefreshSign={() => refresh(currentGame)} />
          </>
        )}
      </div>

      {/* 版本号由后端下发（env!("CARGO_PKG_VERSION")），**禁止硬编码**——
          历史 bug：v1.3.0 的二进制底部仍显示 v1.2.1（前端写死的字符串） */}
      <div className="widget-footer">
        米游社小组件{state?.version ? ` v${state.version}` : ""}
      </div>

      {loginModalOpen && <LoginModal />}
      {settingsOpen && <SettingsModal />}
    </div>
  );
}
