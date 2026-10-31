# YKExplorer 验证记录

日期：2026-10-28  
环境：Windows 10/11 x64、NVIDIA RTX 2060、Rust 1.94.0、GPUI 0.2.2、MSVC 14.44 / Windows SDK 10.0.26100

> 2026-10-28 追加：用户确认标题栏拖动正常、外部拖拽最终成功。  
> 根因有两个：① gpui `DragEnter` 把 `HGLOBAL` 二次解引用后传给 `DragQueryFileW`，拖回自身窗口会崩在 `windows.storage.dll`（已本地 patch）；② 之前从被沙箱标记为 **Low 完整性** 的 `dist` 目录启动，UIPI 阻止 Low → Medium 的 OLE 拖放，Explorer 直接返回禁止光标。改为从普通目录以 Medium 完整性启动后验证成功。
> 最终拖放方案：Shell `IShellItemArray/BHID_DataObject` 数据对象 + UI 线程窗口子类 + 保留鼠标捕获。

> 2026-10-31 追加：修复「从 7-Zip 的压缩包窗口拖文件到本程序，只生成一个 `7zE…` 空文件夹、文件没有复制」。  
> 根因在 7-Zip 的拖放协议：拖动期间 `IDataObject::GetData(CF_HDROP)` 只返回它自己的临时解压目录 `%TEMP%\7zE{HEX}`（`m_hGlobal_HDROP_Pre`），鼠标**松开后**才解压并切换到真实文件列表（`UsePreGlobal = false`），而 `DoDragDrop` 一返回就删除该临时目录。此前 gpui 只在 `DragEnter` 读一次 `CF_HDROP`，所以本程序拿到的是临时目录，异步复制开始时里面还是空的（或已被删除），于是目标处只留下一个空的 `7zE…` 文件夹。  
> 修复：① `Drop` 时重新读取 `CF_HDROP`，拿到真实文件并触发 7-Zip 解压（`vendor/gpui` 的 `FileDropEvent::Submit` 新增 `paths`，`Window` 用它更新当前拖动数据）；② 在 `Drop` 返回前把 `%TEMP%\7zE…` 里的内容 rename 进本程序自己的 `YKExplorer-drop-*` 暂存目录（同盘 rename，瞬时；跨盘或源正在删除时回退为同步复制），后台复制完成后再清理暂存目录，失败则保留并在错误弹窗里给出路径。见 `ops::stage_archiver_drop` / `ExplorerApp::drop_external_paths`。

界面截图：`docs/screenshot.png`（双栏）、`docs/screenshot-grid.png`（宫格）、`docs/screenshot-menu.png`（系统右键菜单）、`docs/screenshot-settings.png`（上下文菜单统一设置页）、`docs/screenshot-shellex-settings.png`（Shell 扩展设置页旧版）、`docs/screenshot-shellex-menu.png`（7-Zip / Beyond Compare 作为二级菜单）、`docs/screenshot-shellex-submenu.png`（7-Zip 二级菜单）。

## 1. 构建

```text
cargo build --release   => Finished (release)
target/release/YKExplorer.exe
dist/YKExplorer.exe
```

本机 MSVC 的 `/Z7` 调试记录问题已通过 `[profile.dev] debug = 0` 规避。

## 2. 自动测试

```text
cargo test
running 24 tests
23 passed; 0 failed; 1 ignored
```

覆盖：

- 文件大小/日期格式化、通配符筛选、文件夹优先、隐藏项过滤
- 复制/移动、冲突自动改名、Windows 非法文件名、可执行扩展名判定
- CF_HDROP UTF-16 路径、`DROPFILES` 头、Preferred DropEffect
- 自定义 `IDataObject` 的 COM 接口级读取
- 自研 `IDataObject` 的 CF_HDROP 读取与跨进程编组（新增）
- 压缩软件临时目录（`7zE{HEX}` / `Rar$…`）识别、拖放内容暂存后归档删除仍可读、普通拖入不被搬动
- 设置默认值、Shell 菜单开关、注册表菜单启用项、JSON 往返
- 注册表菜单作用域路径/稳定 ID、枚举不 panic
- 剪贴板往返测试默认 ignored（无交互桌面时 `OpenClipboard` 返回 `0x80070005`）

## 3. 本轮 UI / 功能实测

按用户要求，本轮不再移动物理鼠标；验证全部通过调试 IPC、`PrintWindow` 和窗口消息完成。

| 项目 | 结果 |
|---|---|
| 设置弹窗分页 | ✅ 常规 / 上下文菜单两页；上下文菜单页用单列表统一展示三类菜单 |
| 回收站删除不闪退 | ✅ `trash` 改为独立线程执行（避免 GPUI 后台线程已有 COM apartment 导致 `RPC_E_CHANGED_MODE` 崩溃）；临时文件移入回收站后进程存活，`crash.log` 无增长 |
| 打开文件不闪退 | ✅ `ShellExecuteW` 移到后台线程并 `CoInitializeEx`；开启替换后用 `open` 打开文件，进程保持存活，`crash.log` 无新记录 |
| 拖出移除 | ✅ 合成消息三组：原位松开 (60,200)→(62,202) 不删除；拖到相邻行 (60,200)→(60,300) 删除「桌面」；拖到文件列表 (60,200)→(700,400) 删除 |
| 拖拽幽灵 | 恢复为 GPUI 圆角卡片：跟随光标，离开原行时边框与「松开移除」变红；指针移出窗口时隐藏，但移除判定仍生效（截图 `docs/screenshot-drag-ghost.png`） |
| 拖拽轮询 | ✅ 合成按下后 60ms 内 `place_drag` 被轮询器结算为 null（按键实际是抬起的），说明跟踪循环在运行并接管了生命周期 |
| 拖拽窗口外 | ✅ 按下时 `SetCapture`，指针离开窗口仍收到事件；窗口外坐标判为移除，幽灵卡片自动隐藏 |
| 点击导航不受影响 | ✅ 抓取后原位轻点仍会跳转（实测点击侧栏项后 `state.path` 变为 `C:\`） |
| 松开丢失兜底 | ✅ 若指针移出窗口后拿不到 mouse-up，下一次移动事件发现按键已抬起即就地结算 |
| 快速访问增删 | ✅ `places add <path>` → 追加「子目录」；`places remove <path>` → 回到 7 项；`places toggle <Desktop>` → 移除/加回；`reset` 恢复默认顺序 |
| 程序图标 | ✅ `build.rs` 生成 7 个尺寸的 `ykexplorer.ico` 并嵌入，`.rsrc` 从 864B 增至 ~21KB；`ExtractAssociatedIcon` 取出的 32px 图标与设计一致（`docs/icon-256.png`） |
| 独立配置文件 | ✅ 旧 `settings.json` 自动迁移为 `%APPDATA%\YKExplorer\config.json`（字体、隐藏项等保持不变），旧文件归档；配置目录只剩 `config.json` |
| 快速访问持久化 | ✅ `settings.json` 写入 `quick_places`（本机桌面在 `F:\UserFile\Desktop`，按真实路径去重） |
| 快速访问设置页 | ✅ 设置新增「快速访问」页签，两列开关列出 7 个默认项 + 恢复默认（截图 `docs/screenshot-quick-settings.png`） |
| 悬停浮起动画 | ✅ 色块从内缩 6/9px、近乎透明开始，200ms 内放大上浮到整行；`hover 3` 稳定后行内与边缘均为 `rgb(228,231,239)`（截图 `docs/crop-float.png`）；临时拉长到 900ms 采样：早期 `rgb(254,254,254)` 且边缘为白 → 结束满色 |
| 侧边栏悬停动画 | ✅ `PostMessage WM_MOUSEMOVE` 悬停到侧栏第 3 项，`state.hovered_side = 2`，截图可见圆角高亮块（`docs/crop-side-hover.png`） |
| 悬停不影响布局 | ✅ 高亮节点始终存在且为绝对定位（不参与 flex gap），文件名/图标位置与修改前一致 |
| 悬停不再抖动 | ✅ 早先"按需插入子节点"会让 hitbox 每帧变化、hover 反复触发；改为常驻节点后 `hover 3` 的 generation 只加 1，连续两次查询 `hovered_row` 都是 3 |
| 侧栏不影响列表动画 | ✅ 列表用 `hover_generation`、侧栏用独立的 `side_generation`，侧栏悬停变化不再重置列表动画 |
| 列表无分隔线 | ✅ 去掉行的 `border_b_1`，选中态改为圆角 `accent_soft` 色块，与侧栏风格一致（截图 `docs/crop-list-selected.png`） |
| 打开文件动画 | ✅ `open` 后截图可见底部提示卡（图标 + `中文说明.txt` + `正在打开…`），160ms 淡入/上移动画，620ms 自动消失 |
| 递归搜索 | ✅ `search .png` 在 demo 下找到 3 个嵌套结果（含 `子目录\深层\image.png`），`search_root` 正确；`search off` 恢复 12 项普通列表；结果按相对路径显示 |
| 撤销：重命名 | ✅ `rename-test a\rename-me.txt|renamed.txt` 后 `undo` 恢复原文件名 |
| 撤销：新建 | ✅ `mkdir-test 撤销测试目录` 后 `undo` 删除该目录 |
| 撤销：复制/移动 | ✅ 复制后用 `undo` 删除副本、源文件保留；移动后用 `undo` 把文件移回原目录 |
| 撤销：回收站 | ✅ `trash` 后 `undo` 通过 `trash::os_limited::restore_all` 从回收站还原文件 |
| Shift 累加框选 | ✅ 已选 4、5 后，按住 Shift 从 1 拖到 2：结果为 1、2、4、5；`pending_drag` 始终为 null，不会触发 OLE 拖出 |
| Ctrl 复制 | ✅ 拖出时用 `GetAsyncKeyState(VK_CONTROL)` 判断复制模式，Shift 不再参与 |
| 空白框选 | ✅ `state` 显示 `rubber_band:true`；蓝色选框覆盖拖拽区域（截图 `screenshot-rubber.png`）；行/卡片在拖拽经过时加入选择 |
| explore 动词 | ✅ 启用后 `ShellExecuteExW(verb="explore")` 与 `verb="open"` 都会启动新的 YKExplorer 进程；停用后 `DelegateExecute` 恢复为 `{11dbb47c-...}` |
| 替换资源管理器 | ✅ 写入 `Folder|Directory|Drive\shell\open\command` 与 `CLSID\{52205fd8-5dfb-447d-801a-d0b52f2e83e1}\shell\opennewwindow\command`；`Start-Process` 打开文件夹、发送 Win+E 均会启动新的 YKExplorer 进程；关闭后原 Folder 命令/DelegateExecute 与 CLSID 键恢复 |
| 启动路径参数 | ✅ 以文件夹路径启动时 `state.path` 指向该目录（Explorer `"%1"` 调用可用） |
| 字体/隐藏文件/菜单开关 | ✅ 开关生效，UI 即时刷新 |
| 设置持久化 | ✅ 写入 `target\debug\ykexplorer.settings.json` 回退文件；重启后 `print=false` 等状态恢复 |
| 系统右键菜单项 | ✅ 打开方式、打印、管理员运行、PowerShell、Windows Terminal、属性按对象类型/开关显示 |
| 菜单管理 | ✅ 关闭“打印”后文件右键菜单不再显示该项；图标/布局正常 |
| 注册表菜单枚举 | ✅ `registry list` 返回 20 项（File 4 / Directory 10 / Background 6），标题、动词、hive 正确 |
| 注册表菜单开关 | ✅ `registry toggle file:vscode` 后设置页开关变蓝，`settings.json` 写入 `registry_menu` |
| 注册表菜单显示 | ✅ 启用“通过 Code 打开”后，文件右键菜单底部出现该项（截图 `screenshot-registry-menu.png`） |
| 上下文菜单页布局 | ✅ 单页滚动列表：内置系统动作（6）、注册表动词（20）、Shell 扩展（COM）三组，分组标题与开关正常（截图 `screenshot-settings.png`） |
| Shell 扩展枚举 | ✅ 过滤系统 DLL 后保留 5 个第三方扩展；7-Zip、Beyond Compare（注册名 `CirrusShellEx`，按 DLL 产品名显示）均正确识别 |
| Shell 扩展功能项 | ✅ `shellex items 7-Zip` 返回 3 个顶层命令 + CRC SHA 子项（CRC-32/SHA-1/SHA-256…），每项都有稳定命令 ID；Beyond Compare 返回“选择左边文件进行比较” |
| Shell 扩展二级菜单 | ✅ 主菜单显示“7-Zip ›”“Beyond Compare ›”；悬停一级项展开二级菜单，点击一级项无动作，点击二级项才执行（截图 `screenshot-shellex-menu.png`、`screenshot-shellex-submenu.png`） |
| Shell 扩展命令调用 | ✅ `shellex invoke 7-Zip` 通过 `InvokeCommand` 执行具体项（启动 7zFM）；未弹出扩展自己的原生菜单 |
| Shell 扩展设置布局 | ✅ 设置页显示程序名、作用域与 hive；7-Zip/Beyond Compare 开关可见（截图 `screenshot-shellex-settings.png`） |
| 设置页图标 | ✅ 新生成 info/print/shield/terminal 图标已加入内嵌资源 |
| 拖入目标 | ✅ 内容区绑定当前目录；驱动器行/快速访问行绑定各自目录（非当前目录），代码层接线完成 |
| 新建入口 | ✅ 列表排满时无空白可点：文件/文件夹右键菜单底部新增「新建文件夹」「新建文件」（带快捷键提示），点击后分别打开对应编辑框（调试 IPC `context-menu` + 截图确认菜单项；点击「新建文件」行得到 `editor=NewFile`）；空白处右键与 `Ctrl+Shift+N` / `Ctrl+Alt+N` 保持不变 |
| 标题栏 | ✅ 用户实测通过；后台光标跟随线程 + `SetWindowPos` |
| 拖出启动 | ✅ 物理光标/左键 watcher + 窗口子类（UI 线程执行 `DoDragDrop`）；日志确认 `window subclass installed` |
| 跨程序拖放 | ✅ 用户实测成功（Explorer → YKExplorer 与 YKExplorer → Explorer 均正常），前提是进程为 Medium 完整性 |
| 从 7-Zip 拖出文件 | ✅ 同一脚本对比：修复前 `dist` 构建只生成空文件夹 `7zE8CBF4E6F`；修复后目标目录得到 `报告.txt`（66 B，内容一致），`YKExplorer-drop-*` 暂存与 7-Zip 的 `7zE…` 临时目录均被清理；日志确认 `DragEnter` 拿到 `...\temp\7zE…`、`Drop` 拿到 `...\temp\7zE…\报告.txt` |
| 拖回自身窗口崩溃 | ✅ 修复 gpui `DragQueryFileW` 传参后不再崩溃 |
| 拖拽首选效果 | ⚠ 标准 Shell 对象不暴露自定义 Preferred DropEffect；允许 MOVE|COPY、Shift 只允许 COPY，最终由目标决定 |
| 注册表菜单调用 | ✅ 代码走 `ShellExecuteExW` 动词；DelegateExecute 类动词可能失败并提示 |
| 调试 IPC | ✅ 仅 debug 构建，loopback 端口；`ping/state/settings/select/context-menu/copy-test` 已实测 |
| 界面无回归 | ✅ 详情/宫格/双栏、搜索、标签、侧栏、排序等原功能布局未变 |

### 标题栏与拖拽

- 标题栏空白区为整段 `flex_1`；鼠标按下后启动短生命周期线程，每 8ms 用 `GetCursorPos` + `SetWindowPos` 跟随移动；用户已实测通过。
- 拖出 watcher 每 16ms 用 `GetAsyncKeyState(VK_LBUTTON)`/`GetCursorPos` 检测，超过 4px 后请求拖放。
- 方案演进：GPUI effect 内调用 → 闪退；Win32 TimerProc → 目标拒绝；独立 STA 线程 → 全部被取消。
- 当前方案：`drag_host` 用 `SetWindowLongPtrW(GWLP_WNDPROC)` 子类化窗口，UI 线程收到 `WM_APP+0x51` 后执行 `DoDragDrop`，此时 GPUI 借用已释放、线程也正确。
- 禁止光标最终定位为 **UIPI**：调试实例从沙箱标记目录启动，进程为 Low 完整性；Explorer 为 Medium，Low→Medium 的 OLE 拖放被系统拦截，目标不会收到数据对象。改为从普通目录启动（Medium）后用户实测成功。
- 同时修复了 gpui `DragEnter` 的 `HDROP` 二次解引用 bug（`vendor/gpui` + `[patch.crates-io]`），否则使用 Shell 数据对象拖回自身窗口会崩在 `windows.storage.dll`。

### 7-Zip 拖出的临时文件夹

复现与修复用的同一套脚本（`tools/e2e_7zip_drag.ps1`，合成鼠标拖拽，7zFM 窗口固定在 (0,0)，条目点在 (40,168)，落点在本程序列表区）：

```text
dist/YKExplorer.exe（修复前）
  dest\7zE8CBF4E6F      <- 只有一个空文件夹，文件没有复制
  temp\                <- 7-Zip 的临时目录已被自己删掉

target/debug/YKExplorer.exe（修复后）
  dest\报告.txt  66 B  <- 内容与压缩包内一致
  temp\                <- 暂存目录与 7-Zip 临时目录都已清理
```

日志（`RUST_LOG=debug`）显示两次 `CF_HDROP` 的内容不同，这正是问题所在：

```text
DragEnter: ["...\temp\7zE4D7CC1FA"]                 <- 占位：7-Zip 的临时解压目录
Drop:      ["...\temp\7zE4D7CC1FA\报告.txt"]         <- 松手后才是真实文件（同时触发解压）
```

对应 7-Zip 源码 `CPP/7zip/UI/FileManager/PanelDrag.cpp`：`CDataObject::GetData` 在 `UsePreGlobal` 为真时返回 `m_hGlobal_HDROP_Pre`（只有 `dirPrefix`），`CDropSource::QueryContinueDrag` 在检测到左键松开后才把 `UsePreGlobal` 置为 false 并设置 `NeedCall_Copy`；解压发生在下一次 `GetData` 里，而 `OnDrag` 结束（即 `DoDragDrop` 返回）就析构 `CTempDir` 删除临时目录。

因此 `Drop` 必须"读一次、搬一次"：重新读 `CF_HDROP` 拿到真实路径，并在返回前把内容 rename 进 `%TEMP%\YKExplorer-drop-<pid>-<stamp>`；否则后续的异步复制一定晚于 7-Zip 的清理。暂存目录与 7-Zip 处在同一个卷（都在 `%TEMP%`），rename 不复制数据，`Drop` 里多出来的开销只是几次改名；真正的耗时仍是 7-Zip 的解压（发生在 `Drop` 内的 `GetData`，解压完成前界面不刷新）和随后的后台复制。

## 4. F 盘“拒绝访问”的定位

用调试 IPC 在运行实例中直接调用同一 `ops::copy_into` 代码：

```text
D:\Project\...\demo\数据.csv -> D:\Project\...\target\test-dest   ✅ 复制成功
D:\Project\...\demo\数据.csv -> C:\Users\...\Temp\ykexplorer-test    ❌ os error 5
D:\Project\...\demo\数据.csv -> E:\__ykexplorer_test                  ❌ os error 5
D:\Project\...\demo\数据.csv -> F:\                              ❌ os error 5
```

同一时刻从 PowerShell 写入 F:\、或由 PowerShell `Start-Process` 启动的 `cmd.exe` / `powershell.exe` 写 F:\ 都成功。
说明当前这次调试运行的应用进程被外层文件沙箱限制为只写工作区 D:，不是 `copy_into` 实现本身拒绝 F。

同时修复了一个真实的 UI 目标错误：

- 之前拖到侧栏 `F:` 行时，根元素仍把文件复制到**当前文件夹**，可能正好落在无写权限目录；
- 现在驱动器行/快速位置行有自己的 `on_drop`，拖到 `F:` 会明确以 `F:\` 为目标，失败弹窗也会显示“目标：F:\...”。

**验收建议**：用资源管理器双击启动 `dist\YKExplorer.exe`（不要在调试沙箱里启动），再试拖到 F 盘/侧栏 F 行。
用户反馈 Windows Explorer 在同类目标会弹出权限确认框，确认后可复制；YKExplorer 目前没有自动提权重试，只显示错误。若需要，应增加“以管理员身份重试”或 IFileOperation 提权路径。

## 5. 手工验收步骤（正常桌面）

### A. 标题栏

在第一个标签右侧、窗口按钮左侧的整段空白按住拖动，窗口应跟随移动。

### B. 默认移动 / Shift 复制

1. 打开 YKExplorer 到 demo 目录，另开资源管理器到目标目录。
2. 把 `中文说明.txt` 拖到资源管理器：源文件应消失（移动）。
3. 恢复文件；按住 Shift 开始拖：源文件应保留（复制）。

### C. 拖到 F 盘

1. 在 F 盘根目录建一个目标目录或确认根目录可写。
2. 从 YKExplorer 把文件拖到侧栏 `F:` 行，松开。
3. 预期：文件复制到 `F:\`，而不是当前标签所在目录。

### D. 系统右键菜单

1. 右键单个 `.exe`：应出现“打开方式…”“以管理员身份运行”“属性”等（按设置开关）。
2. 右键文件夹：应出现 Windows Terminal / PowerShell / 属性。
3. 设置 → 上下文菜单：关闭某一项后重新右键，该项消失。

### E. 注册表右键菜单

1. 在搜索框输入关键词后按 **Enter**，应递归列出子文件夹中的匹配项；按 Esc 返回。
2. 选中项目按 F2 重命名后按 **Ctrl+Z**，应恢复原名；删除到回收站后按 Ctrl+Z 应还原。
3. 打开 **设置 → 常规**，确认“替换系统资源管理器”开关存在；打开后文件夹双击应由 YKExplorer 接管，关闭后恢复。
2. 打开 **设置 → 上下文菜单**，应看到内置动作、注册表动词、Shell 扩展三组；注册表动词按文件/文件夹/空白处分组。
2. 打开某一项的开关（如“通过 Code 打开”）。
3. 回到文件列表，右键对应对象：菜单底部应出现该项；点击应启动对应程序。
4. 关闭开关后该项从右键菜单消失；`settings.json` 的 `registry_menu` 同步更新。

## 6. 已知限制

- 系统右键菜单是常用 Shell 动词子集；注册表菜单同时覆盖 `shell` 动词与 `shellex\ContextMenuHandlers` COM 扩展。
- COM 扩展最多展示两级；更深层的子菜单（如 CRC SHA 内部）拍平到第二级。
- 使用 `DelegateExecute` 的现代动词可能无法由 `ShellExecuteExW` 直接调用，会显示失败提示。
- 从低完整性目录（被沙箱标记）启动时，Windows UIPI 会阻止向 Explorer 等更高完整性进程拖放。
- 拖出没有自定义拖影缩略图。
- 文件系统变更无实时 watcher，需要 F5。
- 文件操作有汇总结果，没有逐字节进度/取消。
- 窗口大小/位置尚未持久化。
- 在远程/已断开桌面或外层文件沙箱中，剪贴板/跨盘写入可能被系统拒绝。

### F. Shell 扩展（COM）

1. 打开 **设置 → 上下文菜单**，滚动到“Shell 扩展（COM）”。
2. 应看到 7-Zip（`7zFM.exe`）、Beyond Compare（`BCompare.exe`）等条目。
3. 打开 7-Zip 与 Beyond Compare 开关。
4. 右键一个文件：菜单底部应出现“7-Zip”“Beyond Compare”。
5. 鼠标**悬停**“7-Zip”应展开二级菜单，列出“添加到压缩包… / 添加到 .7z / 添加到 .zip / CRC SHA …”等具体功能。
6. 点击一级项“7-Zip”不应关闭菜单，也不应执行任何动作。
7. 点击二级菜单中的任一项才执行对应动作；Beyond Compare 同理。
8. 全过程不会弹出扩展自己的原生菜单，也不会把所有功能平铺到主菜单。
