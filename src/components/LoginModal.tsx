import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { api } from "../api";
import { useStore } from "../store";

/** 登录弹窗：扫码（WebView 登录窗） + 手动粘贴 Cookie（受设置开关控制） */
export function LoginModal() {
  const { state, setLoginModal, init, setError } = useStore();
  // 设置中"允许手动粘贴 Cookie"开关：关闭时不展示粘贴 Cookie 入口
  const allowManual = state?.allow_manual_cookie ?? false;
  const [tab, setTab] = useState<"qr" | "cookie">("qr");
  // 登录方式列表：仅一种方式时**不渲染 tab 栏**，改静态提示条——
  //   避免自动化任务把唯一 tab 误判为可点击入口（如误以为点击会打开扫码窗口）
  const tabs: { key: "qr" | "cookie"; label: string }[] = [
    { key: "qr", label: "扫码登录" },
    ...(allowManual ? [{ key: "cookie" as const, label: "粘贴 Cookie" }] : []),
  ];
  const [cookieText, setCookieText] = useState("");
  // 成套导入：App 抓包得到的设备标识（只搬 cookie 会被判 -100，见下方说明）
  const [deviceId, setDeviceId] = useState("");
  const [deviceFp, setDeviceFp] = useState("");
  const [busy, setBusy] = useState(false);
  const [qrMsg, setQrMsg] = useState("");
  const [noteCheck, setNoteCheck] = useState("");

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
        {allowManual ? (
          // 设置中开启了"允许手动粘贴 Cookie" → 展示 tab 切换
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
        ) : (
          // 仅扫码一种登录方式：静态提示条（非按钮、非 tab 外观，自动化无法误判为可交互元素）
          // ⚠️ 2026-09-26 清理：原实现为 `tabs.length > 1 ? tabs : 提示条`，而 tab 状态
          //    只能通过 tab 栏自身切换 → allowManual=false 时 tab 恒为 "qr"，
          //    下方 `tab === "cookie"` 分支为不可达死代码（cookie 分支永远进不去）。
          //    改为直接以 allowManual 分支，语义等价且消除死分支。
          <div className="login-single-method">
            <span className="method-dot" />
            当前仅支持扫码登录
          </div>
        )}

        {tab === "qr" ? (
          <>
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
          </>
        ) : (
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
