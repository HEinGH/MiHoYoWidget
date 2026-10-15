# MiHoYoWidget-Tauri

米游社桌面小组件（Rust + Tauri 2 + React 18）——单进程、免安装的 Windows 小挂件：
扫码登录后展示**每日签到状态**与**游戏便签**，支持开机自启、托盘、钉桌面、透明度与字号调节。

> 从旧版「Python FastAPI + C# WPF 双进程」重构而来，体积由 ~173 MB 降至 ~17 MB，单进程运行。

## 功能状态

| 功能 | 状态 | 说明 |
|---|---|---|
| 扫码登录（米游社 App 扫一扫） | ✅ | WebView 内登录，自动提取 Cookie |
| 每日签到（启动补签 + 周期补签） | ✅ | 服务端权威天数；间隔可配（≥15 分钟） |
| 便签 · 绝区零 | ✅ | 体力/活跃度/周常等，配置驱动渲染 |
| 便签 · 原神 | ❄️ **已冻结** | 见下方「已知限制」 |
| 托盘 / 开机自启 / 单实例 / 钉桌面 / 窗口记忆 | ✅ | |
| 透明度（0%~60%）/ 字体 4 档自动降级 | ✅ | |

## 技术栈

- **后端**：Rust + Tauri 2（`reqwest` / `serde` / `windows` crate）
- **前端**：React 18 + TypeScript + Vite + antd 5 + zustand
- **签名**：自研 DS v1/v2（MD5 + salt；salt 随客户端版本可能变更）
- **凭证**：Windows DPAPI 加密落盘

## 构建与运行

```bash
npm install
npm run tauri dev                 # 开发
npx tsc --noEmit                  # 前端类型检查
npm run tauri build               # 打包（NSIS 安装器需能访问 GitHub）
```

> ⚠️ **只用 cargo 构建正式版时必须带 `custom-protocol` 特征**，否则产物是废包
> （会内嵌 devUrl `http://localhost:1420`，双击报"无法访问此页面"）：
>
> ```bash
> npm run build                                                  # 先生成 dist（编译期会被内嵌）
> cd src-tauri && cargo build --release --features custom-protocol
> ```
>
> 校验产物是否正常：`grep -a -o -c "index-<hash>.js" <exe>` 应 ≥1。

> **发布构建建议加路径重映射** —— 否则 exe 会带上你本机的用户名与 cargo 缓存目录
> （Rust 会把依赖源码的绝对路径写进二进制用于 panic 定位）：
>
> ```powershell
> $env:RUSTFLAGS="--remap-path-prefix=$env:USERPROFILE\.cargo=/cargo"
> npm run tauri build
> ```

## 数据与隐私

程序运行时数据**不在仓库内**，位于用户目录：

| 内容 | 位置 |
|---|---|
| 登录凭证（DPAPI 加密） | `%APPDATA%\com.mihoyo.widget\cookies.enc` |
| 配置（含设备标识） | `%APPDATA%\com.mihoyo.widget\config.json` |
| 调试日志 | `%APPDATA%\mihoyo-widget-debug.log` |
| WebView2 站点数据 | `%LOCALAPPDATA%\com.mihoyo.widget\EBWebView` |

本仓库**不含任何凭证或个人数据**；`.gitignore` 已排除日志、注册表备份、产物与依赖。

## 已知限制

**原神便签不可用（自 v1.3.0 起默认冻结）**
米游社现要求 **App 身份会话**才能读取原神 `dailyNote`：本应用使用网页/通行证身份登录，
请求恒被 `1034`/`5003` 挡下，且自建极验滑块链路提交验证恒 `-1`
（已验证非账号问题、非设备问题、非请求构造问题，详见开发文档）。
为免白白触发风控，默认**不发任何原神便签请求**；设置页留有开关（`genshin_note_enabled`）供实验。

绝区零便签与签到功能不受影响。

## 免责声明

本项目为**个人自用工具**，非米游社官方产品，与米哈游无关联。
请仅用于**你自己拥有**的账号，并自行评估账号风险；因使用产生的任何后果由使用者承担。
