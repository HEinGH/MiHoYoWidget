import { useEffect, useState } from "react";
import { api, FONT_LEVELS } from "../api";
import { useStore } from "../store";

/** 设置弹窗：账号（登录/退出）/ 游戏开关 / 自动签到 / 开机自启 / 透明度 / 字体大小 */
export function SettingsModal() {
  const { state, setSettings, setLoginModal, init, setPreviewOpacity, setError } = useStore();
  const [games, setGames] = useState<string[]>(state?.games ?? []);
  const [autoCheckin, setAutoCheckin] = useState(state?.auto_checkin ?? true);
  const [autoStart, setAutoStart] = useState(state?.auto_start ?? true);
  const [transparency, setTransparency] = useState(state?.transparency ?? 0.9);
  const [fontScale, setFontScale] = useState(state?.font_scale ?? 1.0);
  /** 签到域自动刷新间隔（秒）——下限 15 分钟（后端 `config::update` 会再夹取一次） */
  const [refreshInterval, setRefreshInterval] = useState(state?.refresh_interval ?? 900);
  const [winWidth, setWinWidth] = useState(window.innerWidth);
  const [allowManualCookie, setAllowManualCookie] = useState(
    state?.allow_manual_cookie ?? false,
  );
  // v1.3.0：原神便签开关（默认 false = 冻结）
  const [genshinNoteEnabled, setGenshinNoteEnabled] = useState(
    state?.genshin_note_enabled ?? false,
  );
  const [loginStatus, setLoginStatus] = useState<"checking" | "logged" | "expired" | "none">(
    "checking",
  );
  // 退出登录二次确认弹层开关（防止误操作）
  const [confirmLogout, setConfirmLogout] = useState(false);
  // 重置设备标识二次确认弹层开关
  const [confirmResetDevice, setConfirmResetDevice] = useState(false);

  // ⚠️ 2026-09-26 修复：resize 监听独立成 effect（依赖 []）——
  //    此前它写在依赖 [state] 的 effect 里，每次 state 变化都重复 add 一个监听器；
  //    虽然依赖同一函数引用时会按引用去重，但清理逻辑也被一并重跑，属于隐性脆弱写法。
  useEffect(() => {
    const onResize = () => setWinWidth(window.innerWidth);
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, []);

  useEffect(() => {
    if (!state) return;
    setGames(state.games);
    setAutoCheckin(state.auto_checkin);
    setAutoStart(state.auto_start);
    setTransparency(state.transparency);
    setFontScale(state.font_scale);
    setAllowManualCookie(state.allow_manual_cookie);
    setGenshinNoteEnabled(state.genshin_note_enabled);
    setRefreshInterval(state.refresh_interval);
    // 拉取登录状态
    void api
      .getLoginStatus()
      .then((s) => {
        if (s.valid) setLoginStatus("logged");
        else if (s.has_cookies) setLoginStatus("expired");
        else setLoginStatus("none");
      })
      .catch(() => setLoginStatus("none"));
    // 同步系统开机自启真实状态（与 config 标志保持一致）
    void api
      .getAutoStart()
      .then((v) => setAutoStart(v))
      .catch(() => {});
  }, [state]);

  const update = async (key: string, value: unknown) => {
    await api.updateConfig(key, value);
  };

  const toggleGame = async (id: string) => {
    const next = games.includes(id) ? games.filter((g) => g !== id) : [...games, id];
    if (next.length === 0) return; // 至少保留一个
    setGames(next);
    await update("games", next);
  };

  // 拖动滑条：实时预览（更新全局 previewOpacity，不持久化）
  const changeTransparency = (v: number) => {
    setTransparency(v);
    setPreviewOpacity(v);
  };

  // 选择字体档位：即时应用根字号（实时预览）+ 持久化
  const applyFontScale = (s: number) => {
    setFontScale(s);
    document.documentElement.style.fontSize = `${12 * s}px`;
    void update("font_scale", s);
  };

  /**
   * 设置签到刷新间隔：同步本地状态（App.tsx 的定时器依赖它，需即时生效）+ 持久化。
   * ⚠️ 只影响**签到域**自动刷新；战绩域（便签）永远是"仅手动刷新"，不受此设置影响。
   */
  const applyRefreshInterval = (s: number) => {
    setRefreshInterval(s);
    void update("refresh_interval", s);
    useStore.setState((st) =>
      st.state ? { state: { ...st.state, refresh_interval: s } } : {},
    );
  };

  // 点击"完成"：先持久化当前透明度（await 确保写入完成），再清除预览并刷新状态
  // ⚠️ 2026-08-30 竞态修复：此前 void update() 未等待，init() 的 getAppState 可能先于
  //    update_config 完成 → 读到旧透明度 → 主页面透明度回落（"设置页实色但关掉后主页变透"）
  const applyAndClose = async () => {
    await update("transparency", transparency);
    setPreviewOpacity(null);
    setSettings(false);
    void init();
  };

  // 退出登录（确认弹层"确认"后执行）
  const doLogout = async () => {
    try {
      await api.logout();
      setError("");
      setSettings(false);
      // ⚠️ 立即清空会话数据（不等 init 完成）：
      //    确保主页面不会残留旧账号信息（昵称/UID/便签数据）
      useStore.getState().clearSession();
      void init(); // 刷新为未登录状态（弹登录窗）
    } catch (e) {
      setError(String(e));
    }
  };

  // 二次确认：确认 → 关闭弹层并执行登出；取消 → 仅关闭弹层不执行
  const confirmAndLogout = () => {
    setConfirmLogout(false);
    void doLogout();
  };

  /**
   * 重置设备标识：清除本机米游社站点数据（换新 `_MHYUUID`）与登录凭证，然后**自动重启**。
   * 用途：本应用这个"设备"被服务端标记时（原神便签恒 1034）换一个新设备身份。
   * ⚠️ 会退出登录，重启后需重新扫码。
   */
  const doResetDevice = async () => {
    setConfirmResetDevice(false);
    try {
      await api.resetDeviceIdentity();
      // 正常情况应用会在 ~0.4s 后自动重启，这里一般不会继续执行
    } catch (e) {
      setError(String(e));
    }
  };

  const loginStatusLabel = {
    checking: "检查中...",
    logged: "● 已登录",
    expired: "● 登录已过期",
    none: "● 未登录",
  }[loginStatus];

  return (
    <div className="modal-mask">
      <div className="modal-panel">
        <h3>设置</h3>

        {/* 账号区 */}
        <div className="setting-row" style={{ alignItems: "flex-start" }}>
          <div>
            <div style={{ fontSize: 13 }}>登录状态</div>
            <div
              style={{
                fontSize: 12,
                color:
                  loginStatus === "logged"
                    ? "#66bb6a"
                    : loginStatus === "expired"
                      ? "#ffa726"
                      : loginStatus === "none"
                        ? "#ef5350"
                        : "var(--text-faint)",
              }}
            >
              {loginStatusLabel}
            </div>
          </div>
          <div style={{ display: "flex", gap: 6, flexDirection: "column" }}>
            <button
              className="btn btn-primary"
              style={{ padding: "4px 12px", fontSize: 12 }}
              onClick={() => {
                setPreviewOpacity(null);
                setSettings(false);
                setLoginModal(true);
              }}
            >
              {/* 未登录时按钮语义为"登录"（此前固定"重新登录"，未登录场景语义错误） */}
              {loginStatus === "none" ? "登录" : "重新登录"}
            </button>
            {loginStatus !== "none" && (
              <button
                className="btn btn-ghost"
                style={{ padding: "4px 12px", fontSize: 12 }}
                onClick={() => setConfirmLogout(true)}
              >
                退出登录
              </button>
            )}
          </div>
        </div>

        <div className="setting-row">
          <span>原神</span>
          <button
            className={`switch ${games.includes("genshin") ? "on" : ""}`}
            onClick={() => toggleGame("genshin")}
          />
        </div>
        <div className="setting-row">
          <span>绝区零</span>
          <button
            className={`switch ${games.includes("zzz") ? "on" : ""}`}
            onClick={() => toggleGame("zzz")}
          />
        </div>
        <div className="setting-row">
          <span>自动签到</span>
          <button
            className={`switch ${autoCheckin ? "on" : ""}`}
            onClick={() => {
              setAutoCheckin(!autoCheckin);
              void update("auto_checkin", !autoCheckin);
            }}
          />
        </div>
        {/* 签到刷新间隔：只作用于签到域（api-takumi，无风控）。
            ⚠️ 便签走战绩域（滑动窗口风控），永远是"仅手动刷新"，不在此处配置。 */}
        <div className="setting-row">
          <span>
            签到刷新间隔
            <span style={{ display: "block", fontSize: 10, color: "var(--text-faint)" }}>
              仅影响签到；便签需手动刷新
            </span>
          </span>
          <div className="font-levels">
            {[900, 1800, 3600].map((s) => (
              <button
                key={s}
                className={`font-level ${refreshInterval === s ? "active" : ""}`}
                onClick={() => applyRefreshInterval(s)}
              >
                {s / 60}分
              </button>
            ))}
          </div>
        </div>
        <div className="setting-row">
          <span>开机自启</span>
          <button
            className={`switch ${autoStart ? "on" : ""}`}
            onClick={() => {
              const next = !autoStart;
              setAutoStart(next);
              // ⚠️ 2026-08-30 修复：联动系统注册（autostart 插件），仅写 config 不生效
              void api
                .setAutoStart(next)
                .catch((e) => {
                  setAutoStart(!next); // 失败回滚开关状态
                  setError(`开机自启设置失败: ${e}`);
                });
            }}
          />
        </div>
        <div className="setting-row">
          <span>允许手动粘贴 Cookie</span>
          <button
            className={`switch ${allowManualCookie ? "on" : ""}`}
            onClick={() => {
              setAllowManualCookie(!allowManualCookie);
              void update("allow_manual_cookie", !allowManualCookie);
            }}
          />
        </div>
        <div className="setting-row">
          {/* 透明度语义（0%~60%）：0% = 完全不透明，60% = 40% 不透明（半透）。
              内部 config.transparency 存的是 opacity（1-透明度），见 changeTransparency */}
          <span>透明度 {Math.round((1 - transparency) * 100)}%</span>
          <input
            type="range"
            min={0}
            max={60}
            value={Math.round((1 - transparency) * 100)}
            onChange={(e) => changeTransparency(1 - Number(e.target.value) / 100)}
            style={{ width: 120 }}
          />
        </div>

        {/* 字体大小档位（受窗口宽度限制：小窗口不可选大字号） */}
        <div className="setting-row">
          <span>字体大小</span>
          <div className="font-levels">
            {FONT_LEVELS.map((lv) => (
              <button
                key={lv.scale}
                className={`font-level ${fontScale === lv.scale ? "active" : ""}`}
                disabled={winWidth < lv.minWidth}
                title={winWidth < lv.minWidth ? `窗口过窄（≥${lv.minWidth}px 可选）` : undefined}
                onClick={() => applyFontScale(lv.scale)}
              >
                {lv.label}
              </button>
            ))}
          </div>
        </div>

        {/* v1.3.0：原神便签开关（默认冻结）。冻结 = 便签卡不提供原神刷新入口 + store/后端双拒绝 */}
        <div className="setting-row" style={{ alignItems: "flex-start" }}>
          <div>
            <div style={{ fontSize: 13 }}>原神便签</div>
            <div style={{ fontSize: 10, color: "var(--text-faint)", lineHeight: 1.5 }}>
              {genshinNoteEnabled ? (
                <>
                  已开启：便签卡可对原神手动刷新。
                  <br />
                  米游社现已要求 App 身份会话，网页身份请求会被 1034 挡下。
                </>
              ) : (
                <>
                  已冻结（推荐）：米游社要求 App 身份会话才能读取原神便签，
                  <br />
                  本应用为网页身份，请求必被挡下且验证无法通过，故默认不发请求。
                </>
              )}
            </div>
          </div>
          <button
            className={`switch ${genshinNoteEnabled ? "on" : ""}`}
            style={{ flexShrink: 0 }}
            onClick={() => {
              const next = !genshinNoteEnabled;
              setGenshinNoteEnabled(next);
              void update("genshin_note_enabled", next);
            }}
          />
        </div>

        {/* 维护：重置设备标识（用于摆脱"设备级"风控，例如原神便签恒 1034） */}
        <div className="setting-row" style={{ alignItems: "flex-start" }}>
          <div>
            <div style={{ fontSize: 13 }}>重置设备标识</div>
            <div style={{ fontSize: 10, color: "var(--text-faint)", lineHeight: 1.5 }}>
              清除本机的米游社站点数据（换一个新的设备身份）并重启，
              <br />
              用于摆脱设备级风控。会退出登录，重启后需重新扫码。
            </div>
          </div>
          <button
            className="btn-danger-outline"
            style={{ flexShrink: 0 }}
            onClick={() => setConfirmResetDevice(true)}
          >
            重置
          </button>
        </div>

        <div className="modal-actions">
          <button className="btn btn-primary" onClick={applyAndClose}>
            完成
          </button>
        </div>

        {/* 重置设备标识二次确认 */}
        {confirmResetDevice && (
          <div className="confirm-overlay">
            <div className="confirm-panel">
              <div className="confirm-title">确认重置设备标识？</div>
              <div className="confirm-text">
                将清除本机米游社站点数据（换新设备身份）与本地 Cookie，随后自动重启，
                需要重新扫码登录。仅在本机被风控标记时使用。
              </div>
              <div className="confirm-actions">
                <button className="btn btn-ghost" onClick={() => setConfirmResetDevice(false)}>
                  取消
                </button>
                <button className="btn btn-danger" onClick={() => void doResetDevice()}>
                  重置并重启
                </button>
              </div>
            </div>
          </div>
        )}

        {/* 退出登录二次确认弹层（绝对定位于设置面板内，防止误操作） */}
        {confirmLogout && (
          <div className="confirm-overlay">
            <div className="confirm-panel">
              <div className="confirm-title">确认退出登录？</div>
              <div className="confirm-text">
                退出后将清除本地登录信息（Cookie），需要重新扫码登录才能继续使用。
              </div>
              <div className="confirm-actions">
                <button className="btn btn-ghost" onClick={() => setConfirmLogout(false)}>
                  取消
                </button>
                <button className="btn btn-danger" onClick={confirmAndLogout}>
                  退出登录
                </button>
              </div>
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
