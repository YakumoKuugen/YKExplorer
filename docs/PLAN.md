# FPilot 实施计划表

## 阶段与验收

| # | 阶段 | 工作内容 | 产出 / 验收 | 状态 |
|---|---|---|---|---|
| 0 | 设计与 API 调研 | 读反编译文档、UX 参考；验证 GPUI Windows 后端、字体、文件拖入、OLE 拖出、IME、虚拟列表 | `docs/DESIGN.md`；最小 GPUI 探针成功运行并显示中文 | ✅ |
| 1 | 工程骨架 | Cargo 工程、主题 token、SVG 资源、自绘标题栏/标签栏、窗口控制 | 空壳窗口可启动、可拖动、可缩放、可最小化/最大化/关闭 | ✅ |
| 2 | 文件模型与系统层 | FileEntry、目录枚举、驱动器/已知文件夹、格式化、排序过滤、历史；后台加载 | 侧栏可导航，详情列表展示真实文件，表头排序/过滤生效 | ✅ |
| 3 | 主界面 | 路径栏、动作栏、详情/宫格/双栏、预览、状态栏、Toast | 与参考图一致的完整静态/动态界面 | ✅ |
| 4 | 文本编辑 | 单行编辑器实体、IME、选择、光标、剪贴板、Enter/Esc | 中文搜索、地址栏、重命名无 tofu/不掉字 | ✅ (WM_CHAR 直送验证) |
| 5 | 文件操作 | 打开/定位、复制/剪切/粘贴、新建、重命名、回收站/永久删除、冲突改名 | 日常文件操作完整可用；错误不静默 | ✅ (剪贴板成功路径受会话限制) |
| 6 | 拖放核心 | CF_HDROP 序列化、IDataObject/IDropSource、Shift=复制、外部拖入 | 拖到 Explorer/桌面生效；默认移动、Shift 复制 | ✅ 实现 + 数据对象单测；⚠ 端到端需交互桌面手工验收 |
| 7 | 细节与打磨 | 右键菜单、确认/设置弹窗、快捷键、禁用态、加载态、空状态 | 高频交互顺滑，中文 UI 完整 | ✅ |
| 8 | 测试与交付 | 构建 release、中文回归、截图、README、已知问题 | 可执行文件 + 源码 + 文档 + 截图 | ✅ |
| 9 | 用户反馈修复 | 标题栏移动、拖出可靠性/闪退、标准 Shell 数据对象、侧栏拖入、系统右键菜单、设置分页/持久化、无鼠标调试 IPC | 标题栏、跨程序拖放均已实测通过；F 盘需管理员继续/提权重试 | ✅ |
| 12 | 递归搜索与撤销 | 搜索框 Enter 递归遍历子目录（上限 5000）并按相对路径显示；撤销栈支持重命名/新建/复制/移动/回收站还原（Ctrl+Z、右键菜单） | `search`/`undo` 实测通过；20 个单元测试 | ✅ |
| 11 | 框选与资源管理器替换 | 列表空白处拖拽框选；设置 → 常规增加“替换系统资源管理器”，通过 HKCU 备份/恢复 Directory、Drive 的 open 命令，并支持 `"%1"` 启动路径 | 蓝色选框可选择经过的项；开关可逆，Explorer 双击文件夹打开 FPilot | ✅ |
| 10 | 注册表右键菜单管理 | 枚举 `shell` 动词与 `shellex\ContextMenuHandlers` COM 扩展（`*`/Directory/Folder/Drive/Background），设置页逐项启用；动词用 `ShellExecuteExW`；COM 扩展预构建 `IContextMenu`，以二级菜单列出具体功能并直接 `InvokeCommand` | 设置 → 注册表菜单可管理；7-Zip、Beyond Compare 以二级菜单出现，功能项可区分并执行 | ✅ |

## 目标快捷键

| 快捷键 | 动作 |
|---|---|
| `Alt+←/→/↑`、`Backspace` | 后退 / 前进 / 上一级 |
| `Ctrl+L` / `Ctrl+F` | 地址栏 / 搜索 |
| `Ctrl+T` / `Ctrl+W` / `Ctrl+Tab` | 新建标签 / 关闭标签 / 下个标签 |
| `Enter` / `Ctrl+Enter` | 打开 / 新标签打开文件夹 |
| `F2` / `Ctrl+Shift+N` / `Ctrl+Alt+N` | 重命名 / 新建文件夹 / 新建文件 |
| `Ctrl+C/X/V` / `Delete` / `Shift+Delete` | 复制/剪切/粘贴 / 回收站 / 永久删除 |
| `Ctrl+A` / `Esc` / `F5` | 全选 / 取消与关闭浮层 / 刷新 |
| `Ctrl+1/2/3` | 详情 / 宫格 / 双栏 |
| `Ctrl+B` | 折叠侧栏 |

## 关键技术决策

1. **原生 OLE 拖出而非 GPUI 内部拖拽**：满足“拖到其他程序”的硬需求；拖拽源由 `windows` crate 绑定，`DataObject` 手写，`DoDragDrop` 阻塞在 UI 消息循环中属于 Windows 标准行为。
2. **默认移动、Shift 复制**：拖拽开始时按 Shift 选择允许 effect，并在 `GiveFeedback` 中持续刷新 `CFSTR_PREFERREDDROPEFFECT`；无 Shift 时传 `MOVE|COPY` 保证接收方兼容。
3. **中文原生字体**：不打包字体，不依赖目标机器以外的资源；启动时从系统字体选择微软雅黑/Noto/思源/黑体，回退链覆盖常用 CJK。
4. **不逆向复用**：只参考行为、布局、键位与架构结论；所有图标、颜色、布局、代码重新实现。

## 验收环境

- Windows 10/11 x64
- Rust stable 1.94
- 至少一块 D3D11 兼容 GPU（GPUI Windows 后端要求）
- 中文字体：Microsoft YaHei UI（本机已安装）

## 备注

- 本机 VS 2022 MSVC 在构建 `ring` 时会因 `/Z7` 触发 D8050；工程设置 `[profile.dev] debug = 0` 规避。
- `build.ps1` 只负责 release 构建并把 `target\release\fpilot.exe` 复制为 `dist\FPilot.exe`；不是必要的 vcvars 引导脚本。
- 所有图标/布局/代码均为独立实现，仅参考行为结论，不包含原程序二进制或资源。
