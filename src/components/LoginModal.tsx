import { useCallback, useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { QRCodeSVG } from "qrcode.react";
import { api } from "../api";
import { useStore } from "../store";

type LoginTab = "qr" | "app" | "cookie";

/** 登录弹窗：扫码（WebView 登录窗） + App 会话登录 + 手动粘贴 Cookie（受设置开关控制） */
export function LoginModal() {
  const { state, setLoginModal, init, setError } = useStore();
  // 设置中"允许手动粘贴 Cookie"开关：关闭时不展示粘贴 Cookie 入口
  const allowManual = state?.allow_manual_cookie ?? false;
  const [tab, setTab] = useState<LoginTab>("qr");
  // ⚠️ 2026-10-15：新增第三种登录方式「App 会话登录」后，tab 恒有 ≥2 项
  //    → tab 栏**始终渲染**（原先"仅一种方式时改静态提示条"的分支已不再需要；
  //      该分支的历史目的：避免自动化把唯一 tab 误判为可点击入口）。
  const tabs: { key: LoginTab; label: string }[] = [
    { key: "qr", label: "扫码登录" },
    { key: "app", label: "App 会话登录" },
    ...(allowManual ? [{ key: "cookie" as const, label: "粘贴 Cookie" }] : []),
  ];
  const [cookieText, setCookieText] = useState("");
  // 成套导入：App 抓包得到的设备标识（只搬 cookie 会被判 -100，见下方说明）
  const [deviceId, setDeviceId] = useState("");
  const [deviceFp, setDeviceFp] = useState("");
  const [busy, setBusy] = useState(false);
  const [qrMsg, setQrMsg] = useState("");
  const [noteCheck, setNoteCheck] = useState("");
  // —— App 会话登录状态 ——
  const [appQr, setAppQr] = useState("");
  const [appMsg, setAppMsg] = useState("");
  const [appPhase, setAppPhase] = useState<"idle" | "waiting" | "failed">("idle");

  // 监听扫码登录成功事件
  useEffect(() => {
    const unlisten = listen("login-success", () => {
      setLoginModal(false);
      void init();
    });
    return () => {
      void unlisten.then((fn) => fn());
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // 监听 App 会话登录结果（⚠️ 事件名与扫码登录刻意区分，避免两条链路混淆）
  useEffect(() => {
    const ok = listen("app-login-success", () => {
      setAppPhase("idle");
      setLoginModal(false);
      void init();
    });
    const fail = listen<{ message: string }>("app-login-failed", (e) => {
      setAppPhase("failed");
      setAppMsg(`登录失败：${e.payload?.message ?? "未知原因"}`);
    });
    return () => {
      void ok.then((fn) => fn());
      void fail.then((fn) => fn());
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // 弹窗关闭（组件卸载）时终止后台轮询，避免"关了弹窗还在轮询"的隐性请求
  useEffect(
    () => () => {
      void api.cancelAppLogin().catch(() => undefined);
    },
    [],
  );

  const openQr = async () => {
    setBusy(true);
    setQrMsg("正在打开扫码窗口...");
    try {
      await api.startQrLogin();
      setQrMsg("请在扫码窗口中使用米游社 App 扫描二维码完成登录");
    } catch (e) {
      setQrMsg(`打开扫码窗口失败: ${e}`);
    } finally {
      setBusy(false);
    }
  };

  const startAppLogin = async () => {
    setBusy(true);
    setAppQr("");
    setAppPhase("waiting");
    setAppMsg("正在创建扫码会话...");
    try {
      const res = await api.startAppLogin();
      setAppQr(res.url);
      setAppMsg("请用米游社 App →「扫一扫」扫描二维码，并在手机上确认登录");
    } catch (e) {
      setAppPhase("failed");
      setAppMsg(`创建扫码会话失败：${e}`);
    } finally {
      setBusy(false);
    }
  };

  const cancelAppLogin = useCallback(async () => {
    try {
      await api.cancelAppLogin();
    } finally {
      setAppQr("");
      setAppPhase("idle");
      setAppMsg("");
    }
  }, []);

  const parseCookies = (text: string): Record<string, string> => {
    const map: Record<string, string> = {};
    for (const part of text.split(";")) {
      const idx = part.indexOf("=");
      if (idx <= 0) continue;
      const k = part.slice(0, idx).trim();
      const v = part.slice(idx + 1).trim();
      if (k && v) map[k] = v;
    }
    return map;
  };

  const submitCookies = async () => {
    if (!cookieText.trim()) return;
    setBusy(true);
    setNoteCheck("");
    try {
      const res = await api.loginWithCookies(
        parseCookies(cookieText),
        deviceId.trim(),
        deviceFp.trim(),
      );
      // 导入结果 + 一次性自检结论（这套身份能否读到原神便签）直接展示在弹窗里
      setNoteCheck(res.note_check);
      setLoginModal(false);
      if (!res.note_check.includes("已可读取")) {
        setError(`会话已导入，但${res.note_check}`);
      }
      await init();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="modal-mask">
      <div className="modal-panel">
        <h3>登录米游社</h3>
        <div className="login-tabs">
          {tabs.map((t) => (
            <button
              key={t.key}
              className={`login-tab ${tab === t.key ? "active" : ""}`}
              onClick={() => setTab(t.key)}
            >
              {t.label}
            </button>
          ))}
        </div>

        {tab === "qr" && (
          <div
            style={{
              height: 160,
              display: "flex",
              flexDirection: "column",
              alignItems: "center",
              justifyContent: "center",
              gap: 10,
            }}
          >
            <button className="btn btn-primary" onClick={openQr} disabled={busy}>
              {busy ? "打开中..." : "打开扫码窗口"}
            </button>
            <p className="hint" style={{ textAlign: "center" }}>
              {qrMsg || "使用米游社 App 扫描二维码登录\n登录成功后自动提取 Cookie"}
            </p>
          </div>
        )}

        {tab === "app" && (
          <>
            <div className="app-login-box">
              {appQr ? (
                // ⚠️ 二维码必须由前端渲染：该 URL 对应的官方页面只回静态提示图（实测）
                <div className="qr-box">
                  <QRCodeSVG value={appQr} size={156} level="M" />
                </div>
              ) : (
                <button className="btn btn-primary" onClick={startAppLogin} disabled={busy}>
                  {busy ? "创建中..." : "生成登录二维码"}
                </button>
              )}
              <p className="hint" style={{ textAlign: "center" }}>
                {appMsg || "点击生成二维码，然后用米游社 App 扫一扫"}
              </p>
            </div>
            {/* ⚠️ 透明提示：本登录方式会在通行证侧**登记设备**（官方会发"新设备登录"邮件），
                设备名显示为 MiHoYoWidget。为可追溯性刻意保留此说明，不隐藏副作用。 */}
            <p className="hint">
              此方式走官方「米游社扫码登录」协议，会在你账号的「登录设备管理」中
              <b>新增一台设备</b>（名称 <code>MiHoYoWidget</code>），并可能收到官方的新设备登录提醒邮件。
              它是读取原神便签所需的 App 身份会话的前提。
            </p>
            {appPhase === "waiting" && appQr && (
              <p className="hint">
                二维码有效期约 6 分钟。
                <button className="btn btn-ghost" style={{ marginLeft: 8 }} onClick={cancelAppLogin}>
                  取消
                </button>
              </p>
            )}
          </>
        )}

        {tab === "cookie" && (
          <>
            <textarea
              className="cookie-input"
              placeholder={
                "粘贴 cookie，例如：\nltoken_v2=xxx; ltuid_v2=xxx; cookie_token_v2=xxx; DEVICEFP=xxx"
              }
              value={cookieText}
              onChange={(e) => setCookieText(e.target.value)}
            />
            <input
              className="cookie-input"
              style={{ height: 30, minHeight: 30, marginTop: 6 }}
              placeholder="x-rpc-device_id（App 抓包请求头，建议填写）"
              value={deviceId}
              onChange={(e) => setDeviceId(e.target.value)}
            />
            <input
              className="cookie-input"
              style={{ height: 30, minHeight: 30, marginTop: 6 }}
              placeholder="x-rpc-device_fp / DEVICEFP（可留空）"
              value={deviceFp}
              onChange={(e) => setDeviceFp(e.target.value)}
            />
            <p className="hint">
              来自「米游社 App 抓包」的会话请成套填写：cookie + device_id（+ device_fp），
              三者必须取自同一次抓包 —— v2 会话与设备绑定，只搬 cookie 会被判登录失效(-100)。
              两项留空则沿用当前设备标识（浏览器网页登录的 cookie 可以留空）。
            </p>
            {noteCheck && <p className="hint">自检结果：{noteCheck}</p>}
          </>
        )}

        <div className="modal-actions">
          <button className="btn btn-ghost" onClick={() => setLoginModal(false)}>
            取消
          </button>
          {tab === "cookie" && (
            <button className="btn btn-primary" onClick={submitCookies} disabled={busy}>
              {busy ? "验证中..." : "登录"}
            </button>
          )}
        </div>
      </div>
    </div>
  );
}
