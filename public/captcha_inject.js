// ============================================================
// captcha 窗口注入脚本（在官方 mihoyo 域页面执行）
// 策略：
//   1. 借用官方域身份（Referer/Origin 正确，geetest 白名单放行滑块渲染）
//   2. document.write 立即替换 SPA 为最小验证界面
//   3. 加载 geetest gt.js + initGeetest(product: "float")
//   4. onSuccess 后【浏览器直接 fetch verifyVerification】——
//      WebView2 Chromium TLS 指纹 + 官方域 + 注入 Cookie = 服务端放行
//      （Rust reqwest 原生 TLS 指纹被风控伪装成 retcode=-1 拒绝，已实证）
//   5. 换取 x-rpc-challenge token → emit "captcha-result" 给 Rust 保存
// 参照：paimon-webext（生产可用的米游社便签插件，官方域 + float + 浏览器请求）
// 参数：window.__WGT__（Rust initialization_script 每次文档创建注入，reload 自动刷新）
//       = { game_id, gt, challenge, cookie, device_id, trace }
// ============================================================
(function () {
  "use strict";

  // ---------- 参数 ----------
  function getParam(k) {
    try {
      var u = new URL(window.location.href);
      return u.searchParams.get(k);
    } catch (e) { return null; }
  }
  var WGT = window.__WGT__ || {};
  var INIT = {
    game_id: WGT.game_id || getParam("w_game") || "genshin",
    gt: WGT.gt || getParam("w_gt"),
    challenge: WGT.challenge || getParam("w_challenge"),
    cookie: WGT.cookie || "",
    device_id: WGT.device_id || "",
    device_fp: WGT.device_fp || "",
    trace: WGT.trace || "",
    // 2026-10-14 新增：由 Rust 侧按游戏下发，避免跨游戏绑定错误的 challenge 上下文
    challenge_path: WGT.challenge_path || "",
    signgame: WGT.signgame || "hk4e",
  };

  // 米游社常量（与 paimon-webext / Rust 侧完全一致）
  var DS_SALT = "xV8v4Qu54lUKrEYFZkJhB8cuOh9Asafs";
  var CHALLENGE_PATH = INIT.challenge_path || "https://api-takumi-record.mihoyo.com/game_record/app/genshin/api/dailyNote";
  var VERIFY_URL = "https://api-takumi-record.mihoyo.com/game_record/app/card/wapi/verifyVerification";

  // ---------- md5（blueimp-md5 MIT 精简移植，无依赖） ----------
  var md5 = (function () {
    function safeAdd(x, y) {
      var lsw = (x & 0xffff) + (y & 0xffff);
      var msw = (x >> 16) + (y >> 16) + (lsw >> 16);
      return (msw << 16) | (lsw & 0xffff);
    }
    function bitRotateLeft(num, cnt) { return (num << cnt) | (num >>> (32 - cnt)); }
    function md5cmn(q, a, b, x, s, t) { return safeAdd(bitRotateLeft(safeAdd(safeAdd(a, q), safeAdd(x, t)), s), b); }
    function md5ff(a, b, c, d, x, s, t) { return md5cmn((b & c) | (~b & d), a, b, x, s, t); }
    function md5gg(a, b, c, d, x, s, t) { return md5cmn((b & d) | (c & ~d), a, b, x, s, t); }
    function md5hh(a, b, c, d, x, s, t) { return md5cmn(b ^ c ^ d, a, b, x, s, t); }
    function md5ii(a, b, c, d, x, s, t) { return md5cmn(c ^ (b | ~d), a, b, x, s, t); }
    function binlMD5(x, len) {
      x[len >> 5] |= 0x80 << (len % 32);
      x[(((len + 64) >>> 9) << 4) + 14] = len;
      var i, olda, oldb, oldc, oldd,
        a = 1732584193, b = -271733879, c = -1732584194, d = 271733878;
      for (i = 0; i < x.length; i += 16) {
        olda = a; oldb = b; oldc = c; oldd = d;
        a = md5ff(a, b, c, d, x[i], 7, -680876936);
        d = md5ff(d, a, b, c, x[i + 1], 12, -389564586);
        c = md5ff(c, d, a, b, x[i + 2], 17, 606105819);
        b = md5ff(b, c, d, a, x[i + 3], 22, -1044525330);
        a = md5ff(a, b, c, d, x[i + 4], 7, -176418897);
        d = md5ff(d, a, b, c, x[i + 5], 12, 1200080426);
        c = md5ff(c, d, a, b, x[i + 6], 17, -1473231341);
        b = md5ff(b, c, d, a, x[i + 7], 22, -45705983);
        a = md5ff(a, b, c, d, x[i + 8], 7, 1770035416);
        d = md5ff(d, a, b, c, x[i + 9], 12, -1958414417);
        c = md5ff(c, d, a, b, x[i + 10], 17, -42063);
        b = md5ff(b, c, d, a, x[i + 11], 22, -1990404162);
        a = md5ff(a, b, c, d, x[i + 12], 7, 1804603682);
        d = md5ff(d, a, b, c, x[i + 13], 12, -40341101);
        c = md5ff(c, d, a, b, x[i + 14], 17, -1502002290);
        b = md5ff(b, c, d, a, x[i + 15], 22, 1236535329);
        a = md5gg(a, b, c, d, x[i + 1], 5, -165796510);
        d = md5gg(d, a, b, c, x[i + 6], 9, -1069501632);
        c = md5gg(c, d, a, b, x[i + 11], 14, 643717713);
        b = md5gg(b, c, d, a, x[i], 20, -373897302);
        a = md5gg(a, b, c, d, x[i + 5], 5, -701558691);
        d = md5gg(d, a, b, c, x[i + 10], 9, 38016083);
        c = md5gg(c, d, a, b, x[i + 15], 14, -660478335);
        b = md5gg(b, c, d, a, x[i + 4], 20, -405537848);
        a = md5gg(a, b, c, d, x[i + 9], 5, 568446438);
        d = md5gg(d, a, b, c, x[i + 14], 9, -1019803690);
        c = md5gg(c, d, a, b, x[i + 3], 14, -187363961);
        b = md5gg(b, c, d, a, x[i + 8], 20, 1163531501);
        a = md5gg(a, b, c, d, x[i + 13], 5, -1444681467);
        d = md5gg(d, a, b, c, x[i + 2], 9, -51403784);
        c = md5gg(c, d, a, b, x[i + 7], 14, 1735328473);
        b = md5gg(b, c, d, a, x[i + 12], 20, -1926607734);
        a = md5hh(a, b, c, d, x[i + 5], 4, -378558);
        d = md5hh(d, a, b, c, x[i + 8], 11, -2022574463);
        c = md5hh(c, d, a, b, x[i + 11], 16, 1839030562);
        b = md5hh(b, c, d, a, x[i + 14], 23, -35309556);
        a = md5hh(a, b, c, d, x[i + 1], 4, -1530992060);
        d = md5hh(d, a, b, c, x[i + 4], 11, 1272893353);
        c = md5hh(c, d, a, b, x[i + 7], 16, -155497632);
        b = md5hh(b, c, d, a, x[i + 10], 23, -1094730640);
        a = md5hh(a, b, c, d, x[i + 13], 4, 681279174);
        d = md5hh(d, a, b, c, x[i], 11, -358537222);
        c = md5hh(c, d, a, b, x[i + 3], 16, -722521979);
        b = md5hh(b, c, d, a, x[i + 6], 23, 76029189);
        a = md5hh(a, b, c, d, x[i + 9], 4, -640364487);
        d = md5hh(d, a, b, c, x[i + 12], 11, -421815835);
        c = md5hh(c, d, a, b, x[i + 15], 16, 530742520);
        b = md5hh(b, c, d, a, x[i + 2], 23, -995338651);
        a = md5ii(a, b, c, d, x[i], 6, -198630844);
        d = md5ii(d, a, b, c, x[i + 7], 10, 1126891415);
        c = md5ii(c, d, a, b, x[i + 14], 15, -1416354905);
        b = md5ii(b, c, d, a, x[i + 5], 21, -57434055);
        a = md5ii(a, b, c, d, x[i + 12], 6, 1700485571);
        d = md5ii(d, a, b, c, x[i + 3], 10, -1894986606);
        c = md5ii(c, d, a, b, x[i + 10], 15, -1051523);
        b = md5ii(b, c, d, a, x[i + 1], 21, -2054922799);
        a = md5ii(a, b, c, d, x[i + 8], 6, 1873313359);
        d = md5ii(d, a, b, c, x[i + 15], 10, -30611744);
        c = md5ii(c, d, a, b, x[i + 6], 15, -1560198380);
        b = md5ii(b, c, d, a, x[i + 13], 21, 1309151649);
        a = md5ii(a, b, c, d, x[i + 4], 6, -145523070);
        d = md5ii(d, a, b, c, x[i + 11], 10, -1120210379);
        c = md5ii(c, d, a, b, x[i + 2], 15, 718787259);
        b = md5ii(b, c, d, a, x[i + 9], 21, -343485551);
        a = safeAdd(a, olda);
        b = safeAdd(b, oldb);
        c = safeAdd(c, oldc);
        d = safeAdd(d, oldd);
      }
      return [a, b, c, d];
    }
    function binl2rstr(input) {
      var i, output = "", length32 = input.length * 32;
      for (i = 0; i < length32; i += 8) {
        output += String.fromCharCode((input[i >> 5] >>> (i % 32)) & 0xff);
      }
      return output;
    }
    function rstr2binl(input) {
      var i, length = input.length, output = [], index, length32 = length * 8;
      for (i = 0; i < length32; i += 8) {
        index = i >> 5;
        output[index] |= (input.charCodeAt(i / 8) & 0xff) << (i % 32);
      }
      return output;
    }
    function rstrMD5(s) { return binl2rstr(binlMD5(rstr2binl(s), s.length * 8)); }
    function rstr2hex(input) {
      var hexTab = "0123456789abcdef", output = "", x, i;
      for (i = 0; i < input.length; i += 1) {
        x = input.charCodeAt(i);
        output += hexTab.charAt((x >>> 4) & 0x0f) + hexTab.charAt(x & 0x0f);
      }
      return output;
    }
    function str2rstrUTF8(input) { return unescape(encodeURIComponent(input)); }
    return function (string) { return rstr2hex(rstrMD5(str2rstrUTF8(string))); };
  })();

  // ---------- DS 签名（对齐 paimon getDS：b/q 直接拼接不 md5） ----------
  // paramStr 形如 "is_high=true"（createVerification 用）；verify 无 query 传空
  function getDS(bodyStr, paramStr) {
    var t = Math.floor(Date.now() / 1000);
    // r 范围对齐 genshin.py/Rust 的 100001~200000（与 ds.rs ds_v2 完全一致）
    var r = Math.floor(Math.random() * 100000) + 100001; // 100001 ~ 200000
    var text = "salt=" + DS_SALT + "&t=" + t + "&r=" + r + "&b=" + (bodyStr || "") + "&q=" + (paramStr || "");
    var sign = md5(text);
    return t + "," + r + "," + sign;
  }

  // ---------- Cookie 注入（mihoyo.com 域，fetch 自动携带） ----------
  // ⚠️ 值必须原样写入，不能 encodeURIComponent！编码会把值里的 +/% 等字符改写
  //    （如 + → %2B、% → %25），服务端拿原始值比对直接失败（retcode=-1）
  // ⚠️ HttpOnly cookie（ltoken_v2/cookie_token_v2 等）JS 无法写入也无法读取，
  //    它们必须已存在于 WebView2 cookie 库（扫码登录时种的 .mihoyo.com 域）才能被
  //    fetch 携带；此处仅能注入非 HttpOnly 部分（ltuid_v2/ltmid_v2/account_*）。
  function injectCookies(cookieStr) {
    if (!cookieStr) return;
    var injected = [];
    cookieStr.split(";").forEach(function (pair) {
      var idx = pair.indexOf("=");
      if (idx <= 0) return;
      var k = pair.slice(0, idx).trim();
      var v = pair.slice(idx + 1).trim();
      if (!k || !v) return;
      try {
        document.cookie = k + "=" + v + "; path=/; domain=.mihoyo.com; secure";
        injected.push(k + "(" + v.length + "B)");
      } catch (e) { /* ignore */ }
    });
    showDiag("injected: " + injected.join(","));
    var vis = document.cookie || "(empty)";
    var has = ["ltoken_v2", "ltuid_v2", "cookie_token_v2", "ltmid_v2", "account_mid_v2", "account_id_v2"]
      .map(function (k) { return k + "=" + (document.cookie.indexOf(k + "=") >= 0 ? "yes" : "no"); })
      .join(",");
    showDiag("doc.cookie visible: " + vis.slice(0, 120));
    showDiag("visible keys: " + has);
  }

  // ---------- 浏览器直接调 verifyVerification（Chromium TLS 指纹 + 官方域） ----------
  // ⚠️ 目标：与 Rust create 的头集合**逐字段对齐**（create 是本环境唯一稳定成功的锚点：
  //    GET + is_high=true + DS(q=is_high=true) 全通）。因此这里用 record_headers 同款
  //    （app_version 2.11.1 / client_type 5 / sys_version 12 / platform android /
  //    channel miyousheluodi / signgame hk4e / challenge_path / challenge_trace / device_fp），
  //    【不】用 paimon 2024 模板头（2.75.1 + tool_version + device_name + Origin/Referer）——
  //    2026-08-30 实测：模板头 verify 恒 -1，而 create（2.11.1 系）稳定成功。
  // body 键序 challenge→seccode→validate（ICaptchaRequest 声明顺序，DS 的 b 与 body 字节一致）；
  // 不设 Content-Type（fetch 字符串 body 默认 text/plain;charset=UTF-8）
  function verifyViaBrowser(v, callback) {
    var body = JSON.stringify({
      geetest_challenge: v.geetest_challenge,
      geetest_seccode: v.geetest_seccode || (v.geetest_validate + "|jordan"),
      geetest_validate: v.geetest_validate,
    });
    var deviceId = INIT.device_id || "00000000-0000-0000-0000-000000000000";
    var headers = {
      "x-rpc-app_version": "2.11.1",
      "x-rpc-client_type": "5",
      // ⚠️ device_id 原样传递，不做 toUpperCase：Rust create 用的是 config 里的小写 uuid
      //    （webview 的 _MHYUUID 本身即小写），大写会造成两侧设备标识不一致，
      //    风控按设备匹配时更易被拒（08-30 曾误加大写，当日已回滚，勿再犯）
      "x-rpc-device_id": deviceId,
      "x-rpc-sys_version": "12",
      "x-rpc-platform": "android",
      "x-rpc-channel": "miyousheluodi",
      "x-rpc-signgame": INIT.signgame || "hk4e",
      "x-rpc-challenge_path": CHALLENGE_PATH,
      "DS": getDS(body),
    };
    // device_fp：与 create 同款（本环境 create 带 fp 成功；不带曾触发 10041 设备风控教训）
    if (INIT.device_fp) { headers["x-rpc-device_fp"] = INIT.device_fp; }
    if (INIT.trace) { headers["x-rpc-challenge_trace"] = INIT.trace; }
    // 注：Origin/Referer/sec-fetch-* 由浏览器自动附加（webstatic.mihoyo.com 官方域），
    //     Cookie 由注入 + credentials:"include" 自动携带；UA 浏览器禁改，均不手动设置
    showDiag("[verify] trace=" + (INIT.trace ? INIT.trace.slice(0, 20) + "..." : "(空)"));
    showDiag("[verify] DS=" + getDS(body).split(",")[2]);
    showDiag("[verify] device_id=" + deviceId);
    showDiag("[verify] body=" + body);
    fetch(VERIFY_URL, {
      method: "POST",
      headers: headers,
      credentials: "include",
      body: body,
    })
      .then(function (r) { return r.json(); })
      .then(function (data) {
        if (data && data.retcode === 0 && data.data && data.data.challenge) {
          callback(null, data.data.challenge);
        } else {
          callback("retcode=" + (data ? data.retcode : "?") + (data && data.message ? " " + data.message : "") + " raw=" + JSON.stringify(data));
        }
      })
      .catch(function (e) { callback("fetch error: " + e); });
  }

  // ---------- 备用通道：Rust 侧 verifyVerification（完整模拟 paimon 头集合） ----------
  // 2026-08-30 04:15 实测：浏览器 fetch 恒 -1（UA=WebView2 Edge 无 miHoYoBBS 标识、
  // 缺 device_fp 头等环境特征差异），而 Rust create 一直成功证明 Rust 环境被接受。
  // 浏览器 fetch 失败后自动 fallback 到这里，用 Rust 请求重试（Android miHoYoBBS UA
  // + 显式 Cookie + x-rpc-device_fp + Origin/Referer，与 create 完全一致）。
  function rustVerify(v, callback) {
    if (!(window.__TAURI__ && window.__TAURI__.core && window.__TAURI__.core.invoke)) {
      callback("TAURI invoke 不可用");
      return;
    }
    showDiag("[rust] fallback invoke...");
    window.__TAURI__.core
      .invoke("verify_captcha_fallback", {
        gameId: INIT.game_id,
        geetestChallenge: v.geetest_challenge,
        geetestValidate: v.geetest_validate,
        geetestSeccode: v.geetest_seccode || (v.geetest_validate + "|jordan"),
      })
      .then(function (token) {
        showDiag("[rust] verify ok token=" + String(token).slice(0, 12) + "...");
        callback(null, token);
      })
      .catch(function (e) {
        // 失败上报统一放到"双通道均失败"的分支里做（那里能同时给出浏览器通道的错误），
        // 避免此处先 emit 一次、随后又被合并消息覆盖，造成主窗口提示闪动。
        callback("rust verify: " + e);
      });
  }

  // ---------- 诊断 ----------
  function emit(kind, msg) {
    try {
      if (window.__TAURI__ && window.__TAURI__.event) {
        window.__TAURI__.event.emit("captcha-diagnose", { kind: kind, msg: String(msg) });
      }
    } catch (e) { /* ignore */ }
    try { console.error("[captcha-inject] " + kind + ":", msg); } catch (e) { /* ignore */ }
  }

  // ---------- 状态与诊断（IIFE 顶层定义，供 verifyViaBrowser 等顶层函数调用） ----------
  // ⚠️ 必须放在 IIFE 顶层而非 boot() 内部：verifyViaBrowser 在顶层作用域，
  //    之前 showDiag 只存在于 boot() 局部作用域，verify 内调用直接 ReferenceError，
  //    异常被 create 的 promise .catch 捕获伪装成 "[create] fetch error"，流程卡死
  function setStatus(t, cls) {
    var el = document.getElementById("status");
    if (el) { el.textContent = t; el.className = "status" + (cls ? " " + cls : ""); }
  }
  function showDiag(t) {
    var el = document.getElementById("diag");
    if (el) {
      el.textContent = el.textContent ? el.textContent + "\n" + t : String(t);
    }
    emit("diag", t);
  }
  function heartbeat() {
    try {
      if (window.__TAURI__ && window.__TAURI__.event) {
        window.__TAURI__.event.emit("captcha-loaded", {
          host: window.location.host,
          has_gt: !!INIT.gt,
          has_challenge: !!INIT.challenge,
          has_cookie: !!INIT.cookie,
          has_trace: !!INIT.trace,
          tauri_api: !!window.__TAURI__,
        });
      }
    } catch (e) { /* ignore */ }
  }

  // ---------- 立即替换文档（清除官方 SPA，保留域身份） ----------
  function replaceDoc() {
    var css = [
      "* { box-sizing: border-box; margin: 0; padding: 0; }",
      "body { font-family: 'Microsoft YaHei', system-ui, sans-serif; background: #f5f6f8; min-height: 100vh; display: flex; flex-direction: column; align-items: center; padding: 24px 16px; }",
      ".card { width: 340px; background: #fff; border-radius: 10px; box-shadow: 0 4px 20px rgba(0,0,0,0.08); padding: 20px 16px; display: flex; flex-direction: column; align-items: center; }",
      "h3 { font-size: 15px; color: #1f2329; margin-bottom: 4px; }",
      ".sub { font-size: 12px; color: #8a919f; margin-bottom: 18px; }",
      "#captcha { width: 320px; min-height: 190px; display: flex; justify-content: center; align-items: center; }",
      ".status { margin-top: 14px; font-size: 12px; color: #646a73; min-height: 16px; text-align: center; word-break: break-all; }",
      ".status.ok { color: #00b42a; } .status.error { color: #d54941; }",
      "#diag { margin-top: 6px; font-size: 10px; color: #a9aeb8; min-height: 0; max-width: 320px; text-align: left; white-space: pre-wrap; line-height: 1.5; }",
    ].join("\n");
    var html =
      '<!DOCTYPE html><html lang="zh-CN"><head><meta charset="UTF-8">' +
      '<title>米游社验证码</title><style>' + css + "</style></head><body>" +
      '<div class="card"><h3>米游社验证码</h3>' +
      '<div class="sub">验证通过后自动刷新便签</div>' +
      '<div id="captcha"></div>' +
      '<div class="status" id="status">加载中...</div>' +
      '<div id="diag"></div></div></body></html>';
    try {
      document.open();
      document.write(html);
      document.close();
    } catch (e) {
      window.addEventListener("DOMContentLoaded", function () {
        try { document.body.innerHTML = html; } catch (e2) { /* ignore */ }
      });
    }
  }

  // ---------- 滑块 ----------
  function boot() {
    if (!INIT.gt || !INIT.challenge) {
      setStatus("初始化参数缺失，请关闭后重试", "error");
      showDiag("missing params");
      return;
    }

    // 注入 Cookie（fetch verifyVerification 时自动携带）
    injectCookies(INIT.cookie);
    var ck = INIT.cookie.split(";").map(function (p) { var i = p.indexOf("="); return i > 0 ? p.slice(0, i).trim() : p.trim(); }).filter(Boolean);
    showDiag("cookie keys(" + ck.length + "): " + ck.join(","));
    showDiag("document.cookie: " + (document.cookie ? document.cookie.slice(0, 150) : "(empty)"));
    heartbeat();

    var s = document.createElement("script");
    s.src = "https://static.geetest.com/static/js/gt.0.4.9.js";
    s.onerror = function () {
      setStatus("滑块 SDK 加载失败，请关闭后重试", "error");
      showDiag("gt.js onerror");
    };
    s.onload = function () {
      showDiag("gt.js loaded=" + (typeof window.initGeetest === "function"));
      if (typeof window.initGeetest !== "function") {
        setStatus("滑块 SDK 不可用，请关闭后重试", "error");
        return;
      }
      setStatus("加载滑块...");
      var t0 = Date.now();
      try {
        window.initGeetest(
          {
            gt: INIT.gt,
            challenge: INIT.challenge,
            offline: false,
            new_captcha: true,
            product: "float",
            width: "100%",
            lang: "zh-cn",
            onError: function (msg) {
              setStatus("滑块服务出错: " + msg, "error");
              showDiag("onError: " + msg);
            },
          },
          function (captcha) {
            showDiag("captchaObj created " + (Date.now() - t0) + "ms");
            try {
              captcha.appendTo("#captcha");
              showDiag("appendTo ok");
            } catch (e) {
              showDiag("appendTo threw: " + e.message);
            }
            captcha.onReady(function () {
              setStatus("请点击验证按钮完成验证");
              showDiag("onReady fired, host=" + window.location.host);
              setTimeout(function () {
                var box = document.getElementById("captcha");
                var kids = box ? box.querySelectorAll("*").length : -1;
                var ifr = document.querySelector("iframe");
                showDiag("render check: nodes=" + kids + " iframe=" + (ifr ? "yes" : "none"));
              }, 1500);
            });
            captcha.onSuccess(function () {
              var v = captcha.getValidate();
              if (!v || !v.geetest_validate) {
                setStatus("验证数据获取失败，请重试", "error");
                showDiag("getValidate empty");
                return;
              }
              try { showDiag("validate raw=" + JSON.stringify(v)); } catch (e) { /* ignore */ }
              setStatus("验证通过，正在换取凭证...", "ok");
              showDiag("onSuccess validate=" + v.geetest_validate.slice(0, 12) + "...");
              // ⚠️ 直接 verifyVerification，不再 preflight create！
              //   根因（03:43 实测）：preflight create 用【同一个 trace】二次 createVerification，
              //   服务端会作废滑块正在使用的 challenge（风控会话同 trace 只保留最新挑战），
              //   verify 提交的 validate 绑定失效 challenge → 恒 retcode=-1。
              //   cookie 注入送达已在 03:43 [create] ok 证实，preflight 历史使命已完成。
              function onVerifyOk(token) {
                setStatus("验证成功，正在刷新便签...", "ok");
                showDiag("verify ok token=" + token.slice(0, 12) + "...");
                try {
                  if (window.__TAURI__ && window.__TAURI__.event) {
                    window.__TAURI__.event
                      .emit("captcha-result", { game_id: INIT.game_id, token: token })
                      .then(function () { setStatus("验证成功，窗口即将关闭", "ok"); })
                      .catch(function (e) { setStatus("通知主窗口失败: " + e, "error"); });
                  } else {
                    setStatus("非 Tauri 环境，验证结果已就绪（调试模式）", "ok");
                  }
                } catch (e) {
                  setStatus("通知异常: " + e.message, "error");
                }
              }
              // 双通道：浏览器 fetch 优先（WebView2 Chromium TLS 指纹 + 官方域 + 注入 Cookie）；
              // 失败则 fallback 到 Rust 侧完整请求（与 create 同环境，create 一直成功）
              verifyViaBrowser(v, function (err1, token1) {
                if (!err1) { onVerifyOk(token1); return; }
                showDiag("[browser verify] " + err1);
                setStatus("浏览器验证被拒，尝试备用通道...", "error");
                rustVerify(v, function (err2, token2) {
                  if (err2) {
                    setStatus("验证提交失败：" + err1 + " | " + err2, "error");
                    showDiag("[verify] " + err2);
                    // 双通道均失败：一次上报**两条通道的完整错误**（诊断 -1 类问题必需），
                    // Rust 侧据此解除 trace 冻结并把消息转给主窗口展示
                    try {
                      if (window.__TAURI__ && window.__TAURI__.event) {
                        window.__TAURI__.event.emit("captcha-error", {
                          game_id: INIT.game_id,
                          message: "浏览器通道: " + err1 + " ｜ Rust 通道: " + err2,
                        });
                      }
                    } catch (e3) { /* ignore */ }
                    return;
                  }
                  onVerifyOk(token2);
                });
              });
            });
            captcha.onError(function (e) {
              setStatus("滑块验证出错，请重试", "error");
              showDiag("slider error: " + e);
            });
          }
        );
      } catch (e) {
        setStatus("滑块初始化异常: " + e.message, "error");
        showDiag("initGeetest threw: " + e.message);
      }
    };
    document.head.appendChild(s);
  }

  // ---------- 主流程 ----------
  replaceDoc();
  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", boot);
  } else {
    setTimeout(boot, 100);
  }

  // 窗口复用：Rust 侧复用窗口时下发新参数（reload 保证执行最新代码 + __WGT__ 重新注入）
  try {
    if (window.__TAURI__ && window.__TAURI__.event) {
      window.__TAURI__.event.listen("captcha-init", function (e) {
        var p = e && e.payload;
        if (p && p.gt && p.challenge) {
          try {
            var u = new URL(window.location.href);
            u.searchParams.set("w_game", p.game_id || "genshin");
            u.searchParams.set("w_gt", p.gt);
            u.searchParams.set("w_challenge", p.challenge);
            window.location.href = u.toString();
          } catch (e2) {
            window.location.reload();
          }
        }
      });
    }
  } catch (e) { /* ignore */ }
})();
