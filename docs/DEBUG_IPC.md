# 调试 IPC（仅 debug 构建）

为了避免调试时移动用户鼠标，FPilot 增加了一个**仅 debug 构建启用、默认关闭**的本地命令通道。Release 构建不会编译此模块，也不会监听端口。

## 启动

```powershell
# 方式一：显式命令行参数
cargo run -- --debug-port=47654 "D:\Project\FpilotRec\fpilot\demo"

# 方式二：环境变量
$env:FPILOT_DEBUG_PORT = '47654'
cargo run -- "D:\Project\FpilotRec\fpilot\demo"
```

也可以直接启动：

```powershell
target\debug\fpilot.exe --debug-port=47654 "D:\Project\FpilotRec\fpilot\demo"
```

通道只绑定 `127.0.0.1`。debug 构建的 stderr 会记录 `debug IPC listening on 127.0.0.1:47654`。

## 客户端

项目自带一个最小 PowerShell 客户端：

```powershell
. .\tools\debug_ipc.ps1
Invoke-FpilotDebug 'state'
Invoke-FpilotDebug 'ping'
```

协议非常简单：TCP 一行命令，一行响应。

## 命令

| 命令 | 说明 |
|---|---|
| `ping` | 返回 `pong` |
| `state` | 返回当前路径、视图、选中项、标签页、设置页、右键菜单状态、Shell 菜单开关、注册表菜单数量/启用数等 JSON |
| `navigate <path>` | 导航到目录（不抢前台焦点） |
| `view details\|grid\|split` | 切换视图 |
| `hidden on\|off\|toggle` | 切换隐藏文件 |
| `font <family>` | 切换字体（需在系统字体列表内） |
| `select <name-or-path>` | 选中当前目录中的一项，用于右键菜单测试 |
| `context-menu` | 为当前选中项打开上下文菜单（不移动鼠标） |
| `cancel` | 关闭菜单/弹窗/编辑器并清空拖拽候选 |
| `settings open\|general\|context\|quick\|registry\|close` | 打开设置常规页 / 上下文菜单页 / 快速访问页 / 注册表菜单页，或关闭 |
| `shell-toggle <item>` | 切换系统菜单项并写设置文件 |
| `registry refresh` | 重新枚举注册表右键菜单 |
| `registry list` | 返回枚举结果 JSON（id/scope/verb/title/hive） |
| `config` | 返回当前配置文件路径 |
| `places [add <path>\|remove <path>\|toggle <path>\|reset]` | 查看/修改快速访问条目 |
| `hover <index>\|off` | 走真实 `set_hovered_row` 路径设置/清除悬停行，用于验证高亮滑动动画 |
| `search <query>\|off` | 触发/退出递归搜索 |
| `undo` | 执行一次撤销并返回剩余步数 |
| `rename-test <path>\|<newname>` | 走真实重命名流程（含撤销记录） |
| `mkdir-test <name>` | 走真实新建文件夹流程（含撤销记录） |
| `copy-test <src>\|<dst>[,move]` | 复制/移动并记录撤销 |
| `explorer enable\|disable\|status` | 开启/关闭“替换系统资源管理器”并写设置文件 |
| `rubber [off]` | 用固定坐标显示/关闭框选矩形（调试用，不移动鼠标） |
| `registry toggle <id>` | 切换某个注册表菜单项并写设置文件，例如 `file:vscode` |
| `registry enabled` | 列出已启用的注册表菜单 id |
| `shellex list` | 返回 COM 右键扩展列表 JSON（id/name/clsid/scopes/hive） |
| `shellex items <name>` | 构建指定扩展的隐藏菜单并列出命令 ID 与标签（如 `shellex items 7-Zip`） |
| `shellex invoke <name>` | 用当前选中项执行该扩展的第一个命令（验证 `InvokeCommand`） |
| `submenu <name>` | 打开指定扩展的二级菜单（调试用，不移动鼠标） |
| `shellex toggle <name>` | 切换 COM 扩展开关并写设置文件 |
| `shellex enabled` | 列出已启用的 COM 扩展 id |
| `copy-test <source>\|<dest-dir>` | 同步调用 `ops::copy_into`，用于验证复制/权限问题，不经过鼠标 |
| `list [path]` | 列出目录并返回条目数/名称 |

`copy-test` 示例：

```powershell
Invoke-FpilotDebug 'copy-test D:\Project\FpilotRec\fpilot\demo\中文说明.txt|F:\'
```

输出会包含执行结果、目标路径、成功项和错误项。

## 截图

无需前台焦点或鼠标：

```powershell
. .\tools\capture_hwnd.ps1
$p = Get-Process | Where-Object { $_.Path -eq 'D:\Project\FpilotRec\fpilot\target\debug\fpilot.exe' } | Select-Object -First 1
Save-WindowPng -Handle $p.MainWindowHandle -Path 'D:\temp\fpilot.png'
```

`Save-WindowPng` 使用 `PrintWindow(..., PW_RENDERFULLCONTENT)`，不会移动光标。

## 安全说明

- 仅在 `#[cfg(debug_assertions)]` 下编译；
- 只在显式传入 `--debug-port` 或设置 `FPILOT_DEBUG_PORT` 时启用；
- 仅绑定 loopback，无认证，仅用于本机开发；
- 可用于检查状态、模拟文件和设置操作，但**不会**模拟物理拖放/点击，以免干扰用户。
