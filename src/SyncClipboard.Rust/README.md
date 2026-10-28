# SyncClipboard Lite (Windows 原生极简客户端)

专为 Windows 平台打造的超轻量、低内存、极速剪贴板同步客户端。

## 核心特性
- **极致轻量**：采用纯 Rust 原生 Win32 API 编写，无运行时虚拟机或 GC 开销。物理常驻专用内存仅约 **3 ~ 4 MB**，相比原客户端降低超过 99%。
- **专注双向**：专注于剪贴板**文本与图片**的实时双向同步，零多余冗余功能。
- **毫秒级推送**：内置 WebSocket 接入 SignalR 实时长连接，剪贴板变化即时响应，断线自动重连，配合后台轻量轮询容灾。
- **死循环防震**：基于 SHA256 哈希比对与剪贴板写锁机制，杜绝两端相互回环死循环。
- **原生任务栏托盘与设置**：
  - 任务栏托盘右键菜单支持：状态查看、手动同步、暂停/恢复、配置对话框、退出。
  - 原生 Win32 配置对话框，支持修改服务器地址、用户名、密码与轮询时间，并包含**开机自启动复选框**。
- **平替原客户端**：自动继承 `%APPDATA%\SyncClipboard\SyncClipboard.json` 账号配置，零迁移成本。
- **日志安全上限**：单个日志文件上限 1MB 并自动轮转归档，磁盘占用永远不超过 2MB。

## 构建与运行

### 1. 构建 Release 单文件
```bash
cargo build --release
```
生成单文件二进制：`target/release/syncclipboard-win-rs.exe`。

### 2. 便携运行
可将 `syncclipboard-win-rs.exe` 单文件拷贝到任意目录运行。若需要独立配置，可在同级目录下放置 `config.json`：
```json
{
  "server_url": "https://your-sync-server.com",
  "user_name": "your_username",
  "password": "your_password",
  "pull_interval_sec": 3,
  "enable_push": true,
  "enable_pull": true
}
```
若同级目录下无 `config.json`，程序会自动读取 `%APPDATA%\SyncClipboard\SyncClipboard.json`。
