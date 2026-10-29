# FPilot — 设计说明

> 一个参考 File Pilot 行为与反编译资料、用 **Rust + GPUI** 重写的 Windows 文件资源管理器。
> 不复用原程序的二进制、图标、字体或私有资源；UI 与实现均重新创作。

## 1. 目标

| 编号 | 目标 | 验收方式 |
|---|---|---|
| G1 | GPUI + Rust 原生 Windows 文件管理器 | `cargo build --release` 产出可运行 exe |
| G2 | 原生中文显示与中文输入 | 中文文件名、中文界面、重命名输入无豆腐块 |
| G3 | 从本程序把文件/文件夹拖到其他程序 | 资源管理器、桌面等目标能收到 `CF_HDROP` |
| G4 | 拖出默认“移动”，按住 Shift 拖出为“复制” | 目标端实际执行对应动作，源文件状态正确 |
| G5 | 类似参考图的现代 UI | 标签栏、侧栏、面包屑、详情列表、双栏预览、状态栏 |
| G6 | 可用的日常文件操作 | 打开、导航、多选、复制/剪切/粘贴、重命名、新建、回收站删除 |

### 非目标（本轮）

- 不实现云盘、网络凭据管理、文件搜索索引、压缩包浏览、图片编辑。
- 不追求 1:1 复刻 File Pilot 的全部 556 个快捷键；先覆盖高频操作。
- 不调用或修改安装目录里的 File Pilot。

## 2. 设计语言

### 2.1 布局

```
┌──────────────────────────────────────────────────────────────────────┐
│ [logo] 标签 1 × │ 标签 2 × │ +                       —   □   ×    │ 46px 标题/标签栏
├──────────────────────────────────────────────────────────────────────┤
│ ← → ↑ ⟳   首页 › 用户 › Downloads › 项目      [搜索…]   视图切换    │ 52px 路径栏
├──────────────────────────────────────────────────────────────────────┤
│ ＋ 新建  复制  剪切  粘贴  │ 重命名  删除       排序 ▾   详情/宫格  │ 44px 动作栏
├────────────┬──────────────────────────────────────┬──────────────────┤
│ 快速访问   │ 名称          类型       修改时间  大小 │ 预览 / 详情      │
│ 固定       │ ─────────────────────────────────────  │ 图片或类型图标   │
│ 此电脑     │ 文件行…                               │ 名称/大小/路径   │
│            │                                       │                  │
├────────────┴──────────────────────────────────────┴──────────────────┤
│ 23 项 · 已选 2 项 · 14.8 MB              拖动：默认移动 / Shift 复制 │ 30px 状态栏
└──────────────────────────────────────────────────────────────────────┘
```

- 默认窗口 1440×900，最小 1120×680；窗口使用自绘标题栏，客户区采用平面圆角面板。
- 视图模式：**详情**（主）、**宫格**、**双栏**（详情 + 预览，默认）。参考图的双栏体验。
- 侧栏可折叠（按钮 / `Ctrl+B`），窗口较窄时自动隐藏预览栏。

### 2.2 颜色

| Token | 值 | 用途 |
|---|---|---|
| canvas | `#F5F6F9` | 窗口底色 |
| surface | `#FFFFFF` | 内容面板、列表 |
| sidebar | `#F7F8FA` | 侧栏 |
| border | `#E7E8EC` | 分隔线 |
| text | `#1F2024` | 主文字 |
| text-muted | `#71727A` | 次要文字 |
| text-faint | `#A1A2AA` | 占位、路径分隔 |
| accent | `#2F7CF6` | 主色、焦点、选中描边 |
| selection | `#EAF4FF` | 选中行背景 |
| hover | `#F2F3F6` | 悬停行背景 |
| folder | `#F4B740` | 文件夹图标 |
| danger | `#E5484D` | 删除/错误 |
| success | `#2FA36B` | 成功提示 |

字体：默认使用系统 **Microsoft YaHei UI**，回退链：
`Microsoft YaHei UI → Microsoft YaHei → Noto Sans SC → Noto Sans CJK SC → Source Han Sans SC → SimHei → Segoe UI → Arial`。
启动时从 GPUI `TextSystem::all_font_names()` 里选择第一个可用的中文字体；设置里可切换。字号 13px，行高 20px，标题 14–15px。

图标使用自制 1.5px 描边 SVG（folder、file、search、chevron、grid 等），颜色随文字 token 变化；不依赖彩色 emoji。

### 2.3 组件

- 圆角：8（按钮/输入）、10（卡片）、12（内容面板）、14（弹窗）。
- 行高 36，表头 32；选中行 1px `accent` 描边 + 浅蓝底。
- 图标按钮 28×28，hover 底色；主按钮蓝底白字。
- 标签页：高 34，活跃标签白底+轻阴影，未活跃透明；支持关闭、+ 新建。
- 自绘窗口按钮：最小化、最大化/还原、关闭（关闭 hover 红色）。
- 弹窗：半透明遮罩 + 白面板 + 轻阴影；`Esc` 取消，`Enter` 确认。
- 右键菜单：白底、圆角 10、条目 30px、分隔线，右侧显示快捷键。
- Toast：底部居中，3 秒自动消失，成功/错误两种配色。

## 3. 功能与交互规格

### 3.1 导航

- 侧栏：快速访问（主目录、桌面、下载、文档、图片、音乐、视频）、磁盘（枚举盘符与卷标/容量）。
- 面包屑逐级可点；`Ctrl+L` 或点路径栏空白切换成地址输入（支持 `%USERPROFILE%` 等环境变量、正反斜杠、`~`）。
- `Alt+Left` / `Alt+Right` / `Alt+Up`、退格上一级，`F5` 刷新。
- 多个标签页，每页独立路径、历史、选中项；`Ctrl+T` 新建，`Ctrl+W` 关闭，`Ctrl+Tab` 切换。
- 双击文件夹进入；双击文件调用系统默认程序；`Enter` 同上；`Ctrl+Enter` 在新标签打开文件夹。

### 3.2 选择与排序

- 单击选择；`Ctrl` 切换；`Shift` 范围选择；`Ctrl+A` 全选；`Esc` 取消/关闭浮层。
- 方向键移动光标，`Shift+方向` 扩展选择；Home/End 首尾。
- 表头点击按名称/类型/修改时间/大小排序，再点切换升序/降序；文件夹优先。
- 搜索框实时过滤（名称、类型；支持 `*`、`?` 通配），`Ctrl+F` 聚焦，`Esc` 清空并关闭。

### 3.3 文件操作

- **复制 / 剪切 / 粘贴**：写入 Windows `CF_HDROP` + `CFSTR_PREFERREDDROPEFFECT`；粘贴时支持内部/外部来源，目标冲突自动追加 `(2)`。
- **删除**：`Delete` 移到回收站（服务端操作在后台）；`Shift+Delete` 永久删除，二次确认。
- **重命名** `F2`、**新建文件夹** `Ctrl+Shift+N`、**新建文件** `Ctrl+Alt+N`；输入走自定义单行编辑器，支持 IME 中文。
- **复制路径** `Ctrl+Shift+C`；**在资源管理器中显示** 右键菜单。
- 操作完成后刷新当前目录并显示 Toast；错误用弹窗呈现，不静默失败。

### 3.4 拖出（核心）

- 在文件行/卡片上按下左键后移动超过 4px，启动 **原生 OLE 拖放**（`DoDragDrop`），不是 GPUI 的内部拖拽。
- 数据对象提供：
  - `CF_HDROP`：UTF-16、双 NUL 结尾的路径列表；
  - `CFSTR_PREFERREDDROPEFFECT`：`DROPEFFECT_MOVE` / `DROPEFFECT_COPY`；
  - 通过 `SHCreateStdEnumFmtEtc` 暴露格式。
- 拖拽开始时读取 `Shift`：
  - 未按 → 允许 `MOVE|COPY`，首选 **MOVE**；
  - 按住 → 只允许/首选 **COPY**，保证复制语义。
- `IDropSource::GiveFeedback` 持续读取 Shift 并刷新首选效果；资源管理器、桌面、微信/QQ/浏览器等接受 `CF_HDROP` 的目标都能接收。
- 拖到目标外的空白处/按 Esc 正常取消；拖完返回刷新当前视图。
- 若目标是本程序窗口，GPUI 的 `FileDropEvent` 会提供外部路径；默认复制到当前目录。

### 3.5 外部拖入

- GPUI 0.2.2 Windows 后端已注册 `IDropTarget`，会从 `CF_HDROP` 解析 `ExternalPaths`。
- 在主内容区 `.on_drop::<ExternalPaths>` 接收，复制到当前文件夹；冲突自动改名；完成后刷新。

### 3.6 中文

- 系统性加载 Windows 系统字体，不内嵌字体；确保中文文件名/路径/按钮/输入均使用中文字体回退。
- 重命名、搜索、地址栏使用 GPUI `EntityInputHandler` 实现 IME 组合输入、光标、选择、粘贴。
- 中文输入法的候选框位置通过 GPUI `bounds_for_range` 返回光标矩形。
- 附中文测试目录与回归截图。

## 4. 架构

```
src/
├─ main.rs           Application 初始化、快捷键、打开窗口
├─ app.rs            PilotApp：状态、动作、导航、拖放接线、全部 UI（标题栏/
│                    面包屑/侧栏/详情/宫格/预览/弹窗/菜单/Toast）
├─ fs.rs             目录枚举、元数据、驱动器、用户目录、格式化、排序筛选
├─ ops.rs            复制/移动/回收站/重命名/新建/冲突改名/Shell 打开
├─ clipboard.rs      CF_HDROP 剪贴板读写 + preferred drop effect
├─ drag_out.rs       OLE IDataObject / IDropSource / DoDragDrop
├─ text_input.rs     单行编辑器实体（IME / 选择 / 剪贴板 / 候选窗定位）
├─ assets.rs         内嵌 SVG AssetSource
├─ theme.rs          颜色、字号、字体回退、尺寸
├─ debug_ipc.rs      仅 debug 的本地 TCP 调试通道
└─ tools/            图标生成与离线 UI 自动化脚本（开发用）
```

数据流：`PilotApp` 是唯一根 View；每个标签页保存路径、历史、条目、选中；所有文件任务走 GPUI background executor，完成后用 `WeakEntity` 回主线程更新，防止阻塞 UI。

## 5. API 调研结论（gpui 0.2.2 / Windows）

| 需求 | 结论 |
|---|---|
| Windows 后端 | gpui 0.2.2 内置 DirectX 11 / DirectWrite Windows 后端，`default-features = false, features = ["windows-manifest"]` 可编译运行 |
| 字体 | Windows DirectWrite 后端 + `Font`/`FontFallbacks`；系统字体列表 `TextSystem::all_font_names()`；GPUI 默认 UI 字体已是 Microsoft YaHei UI |
| 外部拖入 | 已内置 `IDropTarget`，事件 `FileDropEvent::{Entered, Pending, Submit, Exited}` → `ExternalPaths` |
| 外部拖出 | **没有**内置 drag source；需自己实现 `IDataObject` + `IDropSource` + `DoDragDrop`，GPUI 已 `OleInitialize`。实现已通过 IDataObject 单元测试 |
| 文本输入 | 无现成单行组件；照 gpui `input` 示例实现 `EntityInputHandler` + 自定义 Element；额外用 IMM32 将候选窗定位到光标 |
| 图片预览 | `img(path)` 支持本地图像，`ObjectFit::Contain/Cover`；仅对选中/可见图片加载，避免全目录解码 |
| 虚拟列表 | `uniform_list`，行/卡片组按需渲染；详情和宫格都可用 |
| 自绘标题栏 | `WindowOptions { titlebar: None }` + `WindowDecorations::Client`；`div().window_control_area(WindowControlArea::Drag)`、`window.minimize_window()/zoom_window()/remove_window()` |
| 右键菜单/弹窗 | GPUI 无 Windows 原生菜单封装，本实现用绝对定位浮层 + 遮罩，保证中文与样式统一 |
| 构建注意 | 本机 MSVC 14.44 在 `ring` 的 `/Z7` 调试记录上报 D8050；设置 `[profile.dev] debug = 0` 后可完整编译 |

## 6. 风险

| 风险 | 缓解 |
|---|---|
| OLE 拖出在目标端的效果与 Shift 语义不一致 | 按 Shift 状态限制允许 effect；拖拽中刷新 preferred effect；用资源管理器/桌面实测并记录 |
| 自绘标题栏在 Windows 丢失缩放边框 | 只在客户区绘制，保留 `WindowDecorations::Client`；实测拖拽边缘与双击最大化 |
| IME 中文输入在 GPUI 的兼容性 | 完整移植官方 input 示例的 `EntityInputHandler`；用中文重命名/搜索实测 |
| 大目录/网络盘卡顿 | 后台线程枚举 + generation 丢弃过期结果 + `uniform_list` |
| 复制大文件夹卡 UI | 文件操作放到 background executor；先实现完成通知与状态提示 |
| GPUI API 快速变化 | 锁定 `=0.2.2`，尽量只使用公开 API |

## 7. 验证清单

- [ ] `cargo build` / `cargo build --release` 通过
- [ ] 启动，中文界面无豆腐块
- [ ] 中文目录名、文件名、路径输入、重命名、中文 IME
- [ ] 多标签、前进/后退/上级、侧栏、面包屑
- [ ] 详情/宫格/双栏、排序、过滤、多选、键盘
- [ ] 复制/剪切/粘贴、新建、重命名、回收站删除
- [ ] 从程序拖文件到 Explorer：默认移动、Shift 复制（手工验收）
- [ ] 从 Explorer 拖文件进程序：复制到当前目录
- [ ] 右键菜单、弹窗、Toast
- [ ] 截图/README

## 后期反馈修复（2026-10-28）

### 1. 无边框窗口移动

GPUI 0.2.2 的 `Window::start_window_move()` 在 Windows 后端是空实现。修复方式：

- 标题栏改为「固定宽度标签条 + `flex_1` 拖拽空白区 + 窗口按钮」；
- 整个标题栏空白区绑定 `on_mouse_down`；
- 鼠标按下时记录屏幕光标与窗口外框位置，启动短生命周期的后台线程；
- 线程每 8ms 读取物理左键/光标并调用 `SetWindowPos` 跟随移动；左键释放即退出；
- 不调用 `WM_NCLBUTTONDOWN`（在 GPUI 的 OLE/消息循环下不可靠），也不依赖 `window_control_area(Drag)`。

### 2. 拖出启动可靠性

之前只在行的 `on_mouse_move` 中检测 4px 阈值。快速拖出窗口时，GPUI 可能不再把鼠标移动事件派发给原行，导致拖拽没有启动。修复：

- `row_mouse_down` 记录物理屏幕光标位置和候选路径；
- 启动一个前台 async watcher，每 16ms 检查 `GetAsyncKeyState(VK_LBUTTON)` 与 `GetCursorPos`；
- 超过 4px 且左键仍按下时，把路径交给 `drag_host::request_drag`；
- `drag_host` 用 `SetWindowLongPtrW(GWLP_WNDPROC)` 给 GPUI 的窗口做子类化，并 `PostMessage(WM_APP+0x51)`；
- 自定义消息在 **UI 线程的窗口过程** 中被处理，此时 GPUI 的 `AppCell` 借用已经释放；
- 在窗口过程里调用 `DoDragDrop`：OLE 需要的消息泵、鼠标捕获与拖拽光标都在 UI 线程，跨进程目标可以正常接收；
- 结果写回全局槽位，watcher 轮询取走后更新 UI/Toast。

三种失败模式的对照：

| 方案 | 现象 | 原因 |
|---|---|---|
| GPUI effect 内直接 `DoDragDrop` | 闪退 | OLE 模态循环重入 GPUI，`RefCell already borrowed` |
| Win32 TimerProc | 目标显示禁止光标、最终取消 | 拖放在错误线程执行，跨进程数据对象不被接受 |
| 独立 STA 线程 | 所有目标都被取消 | OLE 拖放必须在源窗口线程执行，源线程不再泵消息 |
| **窗口子类 + UI 线程** | ✅ | 借用已释放且线程正确 |

### 3. 侧栏拖入与 F 盘

原先整个窗口的 `on_drop` 只把 `ExternalPaths` 复制到当前文件夹，拖到侧栏 `F:` 也走当前目录，容易触发目标目录权限错误。现在：

- 文件列表内容区负责“复制到当前文件夹”；
- 驱动器行与快速访问行各自绑定 `on_drop::<ExternalPaths>`；
- 拖到 `F:` 行时目标为 `F:\`，不再误用当前目录；
- 失败弹窗显示「目标：...」并附加权限提示；
- 设计上仍是“外部拖入 = 复制”，不擅自删除源文件。

> 注意：本次验证的调试实例运行在外层文件沙箱中，只有 D: 可写；F:/E:/C:\Temp 均被拒绝。
> 正常桌面启动的 `dist\FPilot.exe` 不受这个外层限制，应由用户按 README 手工验证 F 盘。

### 4. 系统右键菜单子集与设置

- 通过 `ShellExecuteExW` 发起 Windows 原生动词：`openas`、`print`、`runas`、`properties`；
- PowerShell/Windows Terminal 通过子进程启动；
- 菜单项根据右键对象类型、可执行扩展名与设置开关动态显示；
- 设置弹窗分为「常规 / 上下文菜单」两页；上下文菜单页用单列表统一管理内置动作、注册表动词和 Shell 扩展，开关保存到
  `%APPDATA%\FPilot\settings.json`，失败时回退到 exe 同目录 `fpilot.settings.json`；
- 字体、隐藏文件、替换资源管理器、内置动作、注册表动词、Shell 扩展启用项一并持久化。
- 替换资源管理器只写 HKCU：`Software\Classes\Folder|Directory|Drive\shell\open\command` 以及 `shell\explore\command`（`explore` 与 `open` 是两个独立动词，`ShellExecute` 显式传 "explore" 时走后者），并把原命令、值类型、DelegateExecute、键是否存在备份到 `Software\FPilot\ExplorerBackup`；HKLM 仍会提供 Explorer 的 `DelegateExecute`，因此启用时把它覆盖为无效值 `FPilotDisabled`，Shell 会回退到 command；关闭时按原类型恢复 `Folder` 的 `%SystemRoot%\Explorer.exe` 与 DelegateExecute，或删除自建的 Directory/Drive 键。
- 额外接管 `HKCU\Software\Classes\CLSID\{52205fd8-5dfb-447d-801a-d0b52f2e83e1}\shell\opennewwindow\command`。该 CLSID 的显示名就是 “File Explorer”，开始菜单的“文件管理器”和 Win+E 都走它的 `OpenNewWindow`；这是 File Pilot 的做法。
- 图标：`assets/icons/explorer.svg` 是唯一来源，`build.rs` 用 `resvg`（build-dependency）按 7 个尺寸光栅化并写入 ICO，再用 `rc.exe` 生成 `.res` 通过 `cargo:rustc-link-arg-bins` 链入 exe；找不到 `rc.exe` 时静默跳过。
- 配置：单一文件 `%APPDATA%\FPilot\config.json`（`dirs::config_dir()`），不再往程序目录写配置，因此重装不影响；首次启动若没有 `config.json`，会依次尝试读取旧的 `%APPDATA%\FPilot\settings.json` 与程序目录的 `fpilot.settings.json`，成功后写入 `config.json` 并将旧文件改名为 `*.json.migrated`。字段全部带 `#[serde(default)]`，新增设置项不会让旧配置失效。
- 快速访问：`quick_places` 是 `Vec<QuickPlace>`，持久化为路径数组；`fs::quick_place_for()` 对内置路径复用原有名称/图标，其余用文件夹名。侧栏条目按下时调用 `SetCapture` 抓取鼠标（否则指针离开窗口就收不到移动/松开），记录 `place_drag`、起点与当前位置，拖动期间由 `track_place_drag` 每 16ms 轮询 `GetCursorPos`（GPUI 在指针离开窗口后不再派发 move，不能依赖事件），据此更新幽灵位置与「是否离开原行」，并用 `GetAsyncKeyState(VK_LBUTTON)` 判断按键抬起——即使松手在窗口外也能结算。幽灵仍是 GPUI 元素（圆角卡片：图标 + 名称 + 状态提示，离开原行时边框与提示变红），由轮询到的光标位置驱动；指针移出窗口时卡片自动隐藏（窗口无法绘制到自身之外），但拖拽判定与移除仍然生效；结算以松开坐标为据——x ≤ 232 且 |Δy| ≤ 18（即回到原来那一行）取消，其余情况（含窗口外负坐标）移除；结算后 `ReleaseCapture`；指针在窗口外时隐藏幽灵卡片。若 mouse-up 仍丢失，下一次移动发现按键已抬起即就地结算，不再依赖移动事件——GPUI 的 move 只在指针仍在该元素上时才派发，拖出行外就收不到。行内 mouse-up 与根节点兜底都用事件位置判断，避免先命中行内处理器时吞掉手势；侧栏滚动区与分组标题都是拖放目标：拖入的都是文件夹则固定到快速访问，否则沿用原来的复制到该位置。设置页只列出 `fs::quick_places()` 的默认项，按路径判断开关状态。
- 悬停高亮：`hovered_row` / `hover_prev`（列表）与 `hovered_side` / `hover_prev_side`（侧栏）记录光标进出，分别用 `hover_generation` 与 `side_generation` 计数，避免互相互相重置动画。`slide_block()` 始终返回一个**绝对定位**色块：空闲时透明度 0，悬停时用 `with_animation`（200ms，`ease_out_quint`）把上下左右内缩量从 6/9px 收到 0、透明度 0→1，形成"从后方浮起"的效果。节点常驻且类型一致（都是 `AnimationElement<Div>`），避免 element tree 变化导致 hitbox 抖动、hover 事件反复触发。选中行传入 `selected_tint`，渲染为静态圆角色块（`accent_soft`）；列表通过 `side_inset = 6px` 让色块左右各留一段空白（与侧栏一致），行本身去掉底部分隔线。
- 打开反馈：`open_flash` 保存 `(文件名, 计数)`，渲染为底部居中卡片，`with_animation` 做 160ms 淡入 + 上移，定时器 620ms 后清除；必须 `cx.notify()` 触发首帧。
- 递归搜索：`fs::search_directory` 用队列广度遍历，跳过符号链接目录避免环，命中上限 5000 条；结果直接替换 `TabState::entries`，`search_root` 保留根目录用于显示相对路径，导航或 Esc 时清除并重新加载原目录。
- 撤销：`Vec<UndoAction>` 保存 `Rename`/`Created`/`Moved`/`Trashed` 四类操作（上限 64 条）。`OpReport::moved` 记录移动的源与目标；回收站还原用 `trash::os_limited::list` + `restore_all` 按原路径匹配，并在独立线程执行以避开 COM apartment 冲突。
- 按住 Shift 在条目上按下：清空原有选择、只选中起点那一项，并把 `rubber_band` 置为起点；同时跳过 `pending_drag`，因此不会开始 OLE 拖出。拖出改由 Ctrl 表示复制。
- 空白处按下左键开始框选，`rubber_band` 保存窗口坐标；行/卡片在鼠标经过且左键仍按下时加入选择，根节点绘制半透明选框，松手清除。

#### 注册表右键菜单

- 启动时（以及设置页「刷新列表」）枚举 `HKCU/HKLM\Software\Classes\*\shell`、
  `Directory\shell`、`Directory\Background\shell` 下的子键；
- 读取 `MUIVerb` 或默认值作为标题，`@dll,-id` 形式通过 `SHLoadIndirectString` 解析；
  跳过带 `LegacyDisable` 的项，同一动词 HKCU 覆盖 HKLM；
- 稳定 ID 形如 `file:vscode`、`dir:git_shell`、`background:cmd`，开关状态存入
  `settings.registry_menu`（只记录启用项）；
- 右键菜单按对象作用域匹配启用项：文件 → File，文件夹 → Directory，空白处 → Background；
  点击后调用 `ShellExecuteExW`，`lpVerb` 为注册的动词名；
- 设置页按作用域分组、滚动展示，提供「刷新列表」「全部关闭」。

#### Shell 扩展（COM）

- 枚举 `shellex\ContextMenuHandlers`：`*`、`Directory`、`Folder`、`Drive`、
  `Directory\Background`，覆盖 HKLM/HKCU 及 Wow6432Node；
- 按显示名合并同一扩展的多个作用域；显示名优先取扩展 DLL 版本信息的 `ProductName`
  （Beyond Compare 的注册名实际是 `CirrusShellEx`），并去掉 “Shell Extension” 后缀；
- 只保留在当前 64 位进程中存在 `InprocServer32` 的 CLSID；跳过位于 `%SystemRoot%`
  的系统处理器；
- 注册表只保存 DLL 路径，没有命令行；功能项由扩展的 `IContextMenu` 在运行时生成；
- 打开 FPilot 右键菜单时，对每个启用的扩展：
  `CoCreateInstance` → `IShellExtInit::Initialize(folder pidl, shell data object)`
  → `IContextMenu::QueryContextMenu` 到隐藏 `HMENU`，递归读取菜单项与命令 ID；
- FPilot 只渲染两级：主菜单中是扩展名（如“7-Zip ›”“Beyond Compare ›”），
  鼠标悬停一级项展开二级菜单，点击一级项不执行动作；更深的子菜单（CRC SHA 内部）拍平到二级；
- `HandlerMenu` 在菜单打开期间持有 `IContextMenu` 与 `HMENU`；点击具体项时
  直接 `InvokeCommand`（命令 ID），不显示扩展自己的原生菜单；
- 菜单关闭后释放；设置页显示程序名、作用域与 hive，开关保存到 `settings.shellex_menu`。

### 5. 无鼠标调试

新增仅 debug 构建启用的本地 TCP 调试通道（`--debug-port` / `FPILOT_DEBUG_PORT`），
支持 state/navigate/view/settings/select/context-menu/copy-test/list 等命令；
配合 `PrintWindow` 截图，不再需要移动用户鼠标。

### 6. 拖拽闪退修复（2026-10-28 追加）

复现到的 panic：

```text
thread 'main' panicked at gpui-0.2.2/src/app/async_context.rs:57:
RefCell already borrowed
...
DoDragDrop
fpilot::drag_out::start_file_drag
gpui::app::App::flush_effects
```

原因：在 GPUI effect / async task 持有可能的 AppCell 借用时调用 `DoDragDrop`；OLE 模态循环重入 GPUI，
再次 `borrow_mut` 即 panic，Windows GUI 进程没有控制台，表现为闪退。

修复过程：
1. GPUI effect 内直接调用 → `RefCell already borrowed` 闪退；
2. Win32 TimerProc → 不再闪退，但目标显示禁止光标、拖放被取消；
3. 独立 STA 线程 → 所有目标都被取消（OLE 拖放必须在源窗口线程运行）；
4. 最终方案：**窗口子类 + 自定义 `WM_APP` 消息 + UI 线程执行 `DoDragDrop`**。

另外加了 panic hook 与 `SetUnhandledExceptionFilter`，崩溃会写 `crash.log`。

随后用户反馈“其他程序显示禁止光标”。最终定位到两个独立原因：

1. **gpui 的 `DragEnter` bug**：它把 `idata.u.hGlobal` 二次解引用后传给 `DragQueryFileW`，
   使用 Shell 数据对象拖回自身窗口时会崩在 `windows.storage.dll`。已将 gpui 复制到
   `vendor/gpui` 并用 `[patch.crates-io]` 接入，修正为直接传 `HDROP(hGlobal)`。
2. **UIPI**：调试实例从被沙箱标记为 Low 完整性的 `dist` 目录启动，进程为 Low；
   Explorer 为 Medium。Windows 阻止 Low → Medium 的 OLE 拖放，目标不查询数据对象直接返回
   `DROPEFFECT_NONE`。改为从普通目录启动（Medium）后，用户实测 FPilot → Explorer 成功。

最终拖放方案：**Shell `IShellItemArray -> BHID_DataObject` 数据对象 + UI 线程窗口子类 +
保留源窗口鼠标捕获**；自研 `CF_HDROP` 对象保留为回退。
