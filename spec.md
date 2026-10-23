# Faz 功能需求清单

本文档列出 Faz 当前**已实现**的功能需求，每条都可追溯到代码位置。它不是路线图，也不是产品设想——写进来的功能都在主分支上可用，且标注了验证方式。

版本基准：`config_version 34`，五个顶层菜单（Faz / Shell / Edit / View / Window）。

---

## 1. 终端核心

### 1.1 PTY 与会话

| 编号 | 需求 | 状态 | 验证 |
|---|---|---|---|
| T-101 | 每个 pane 拥有独立 PTY，本地 shell 运行在 slave 上 | 已实现 | `mux/src/domain.rs` |
| T-102 | pane 输出经 socketpair 交给解析线程，与 PTY 读取解耦 | 已实现 | `mux/src/lib.rs` `read_from_pane_pty` |
| T-103 | PTY 读错误中 `EAGAIN`/`EINTR`/`TimedOut` 视为可重试，不作致命处理 | 已实现 | `is_retryable_read`，单测覆盖 |
| T-104 | PTY 真正失效（`EBADF`/`EIO`）时显示提示并强制移除 pane | 已实现 | `mux/src/lib.rs` `pty_fatal` |

### 1.2 终端语义

| 编号 | 需求 | 状态 | 验证 |
|---|---|---|---|
| T-201 | VT 序列解析、屏幕缓冲、滚动历史 | 已实现 | `term/` |
| T-202 | 同步输出（Synchronized Output）原子渲染 | 已实现 | `SynchronizedOutputState`，单测覆盖 |
| T-203 | 选区、复制、粘贴、鼠标追踪 | 已实现 | `term/`、`faz-gui/src/termwindow/` |
| T-204 | 超链接识别：URL、路径、裸域名 | 已实现 | `docs/features.md` |
| T-205 | Option+Click 在当前输入行内移动光标 | 已实现 | `docs/features.md` |
| T-206 | 中日韩宽字符与组合字符渲染 | 已实现 | `termwiz/`、字体 shaper |

---

## 2. 窗口与布局

| 编号 | 需求 | 状态 | 验证 |
|---|---|---|---|
| W-101 | 多 tab、多 pane、水平/垂直分屏 | 已实现 | `mux/src/tab.rs` |
| W-102 | pane 缩放（zoom）、pane 导航与选择器 | 已实现 | `TogglePaneZoomState`、`paneselect.rs` |
| W-103 | 多窗口，窗口位置尺寸持久化 | 已实现 | `window/src/os/macos/window.rs` |
| W-104 | 窗口快照：关闭/隐藏时保存布局，可从 Shell 菜单恢复 | 已实现 | `docs/features.md` |
| W-105 | 全屏、居中、填满、置顶/置底 | 已实现 | View 菜单 |
| W-106 | 标签页重命名 | 已实现 | `tab_rename.rs` |
| W-107 | 标签栏位置与样式可配置 | 已实现 | `config/` |

---

## 3. 配置系统

| 编号 | 需求 | 状态 | 验证 |
|---|---|---|---|
| C-101 | Lua 配置，支持热重载 | 已实现 | `config/src/lib.rs` |
| C-102 | `config_version` 随发布递增，触发已安装用户重新生成 shell 集成 | 已实现 | `assets/shell-integration/config_version.txt`、`scripts/check_release_config.sh` |
| C-103 | 旧配置键通过 deprecated 字段容忍，**不做版本迁移** | 已实现 | `config/src/config.rs` 的 `deprecated = "..."` |
| C-104 | 配置 TUI（`faz config`）进行交互式设置 | 已实现 | `faz-cli/src/config_cmd.rs` |
| C-105 | 自定义键绑定，**必须追加**而非替换 `config.keys` | 已实现 | `docs/configuration.md` |
| C-106 | 主题与配色 | 已实现 | `config/`、`docs/configuration.md` |
| C-107 | 系统代理检测；内网/回环地址直连 | 已实现 | `config/src/proxy.rs` |

**约束**：不引入 `config.language` 式的 UI 多语言。历史上 `rust-i18n` 方案已回退，UI 字符串保持英文。

---

## 4. Shell 集成

| 编号 | 需求 | 状态 | 验证 |
|---|---|---|---|
| S-101 | zsh / bash / fish 集成脚本安装（`faz init`） | 已实现 | `assets/shell-integration/` |
| S-102 | 通过 user-var 向终端报告 cwd、git 分支、命令状态 | 已实现 | `assets/shell-integration/` |
| S-103 | shell 集成可重置（`faz reset`） | 已实现 | `faz-cli/src/reset.rs` |
| S-104 | 生成的脚本通过 shellcheck 与 `zsh -n` 语法检查 | 已实现 | CI `checks.yml` |
| S-105 | zsh 与 bash 都在 preexec 时发布 `WEZTERM_PROG`（base64），提示符处清空 | 已实现 | `setup_zsh.sh`、`faz.sh` |
| S-106 | 自定义 `format-tab-title` 优先于内置 SSH 标题（包括 hover 路径） | 已实现 | `tabbar.rs` `is_builtin_ssh_title`，单测覆盖 |

**约束**：shell 集成脚本在用户 shell 中运行，不只是构建期。Bash heredoc 生成 zsh 时必须转义反引号和 `$(...)`。

---

## 5. 命令面板与浮层

| 编号 | 需求 | 状态 | 验证 |
|---|---|---|---|
| U-101 | 命令面板：按名称/描述检索并执行任意 `KeyAssignment` | 已实现 | `overlay/launcher.rs` |
| U-102 | QuickSelect：屏幕文本快速选中 | 已实现 | `overlay/quickselect.rs` |
| U-103 | 字符/Emoji 选择器 | 已实现 | `charselect.rs` |
| U-104 | 通用确认浮层与输入提示浮层 | 已实现 | `overlay/confirm.rs`、`overlay/prompt.rs` |
| U-105 | 浮层全屏独占；浮层期间按键不进底层 pane | 已实现 | `get_active_pane_or_overlay` |

---

## 6. ZMODEM 文件传输

### 6.1 已实现能力

| 编号 | 需求 | 状态 | 验证 |
|---|---|---|---|
| Z-101 | 检测远端 `rz`/`sz` 发起的传输会话 | 已实现 | `mux/src/zmodem/detect.rs` |
| Z-102 | 文件选择器挑选发送文件 / 保存目录；空格或 Ctrl+S 确认目录 | 已实现 | `overlay/filebrowser.rs` |
| Z-119 | 文件选择器显示隐藏文件（`.config` 等路径可直接浏览与补全） | 已实现 | `read_dir`，单测 `read_dir_includes_hidden_entries` |
| Z-120 | `Ctrl-P` 进入路径输入行：Tab 补全、方向键编辑、`PageUp`/`PageDown` 翻候选、`Esc` 退出输入行 | 已实现 | `handle_input_event`、`complete_path`，单测覆盖 |
| Z-121 | 补全支持 `~` 展开、相对路径、绝对路径；目录补全补尾斜杠；目录模式只补目录 | 已实现 | `complete_path`、`resolve_against`，单测覆盖 |
| Z-122 | 按显示列宽排版（CJK 占 2 列）：截断、候选列宽、光标位置均按单元格计算，避免折行错位 | 已实现 | `truncate`、`pad_to_cells`，单测覆盖 |
| Z-123 | 候选多行面板：按列排布、可翻页、显示范围与总数，占独立区域不覆盖列表 | 已实现 | `completion_columns`、`completion_page` |
| Z-124 | 每个 pane 独立记录待处理的 ZMODEM 会话，广播模式下多 pane 同时 `rz` 互不覆盖 | 已实现 | `zmodem_pending` 按 pane 记录，单测 `zmodem_pickers_are_per_pane` |
| Z-125 | 文件选择器在所属 pane 内绘制，不占满整个 tab；多个 pane 可同时各持一个选择器 | 已实现 | `start_overlay_pane` + `assign_overlay_for_pane` |
| Z-126 | 路径输入历史：同一次运行内 `↑`/`↓` 回溯已确认路径，不持久化 | 已实现 | `zmodem_pick_history` |
| Z-127 | 选择器记忆上次位置，按 pane 与方向分别记录 | 已实现 | `zmodem_last_pick` |
| Z-103 | 取消选择时向远端发送中止，并立即允许再次发起 | 已实现 | `finish_cancelled`，单测覆盖 |
| Z-104 | 传输期间 PTY 读取权独占，另一读者阻塞等待 | 已实现 | `ReadGate`，单测覆盖 |
| Z-105 | 传输期间拒绝所有非传输写入（按键、粘贴、注入、图片粘贴、字符写入），Ctrl+C 例外 | 已实现 | `is_zmodem_exclusive`，6 条路径全覆盖 |
| Z-106 | 被拒绝的按键回显到屏幕（不写入 PTY） | 已实现 | `key_echo`，单测覆盖 |
| Z-107 | 上传时以 `sz -e` 运行，转义全部控制字节 | 已实现 | 单测 `escape_flag_tests` |
| Z-108 | PTY 特殊字符槽清空，`VMIN`/`VTIME` 保留 | 已实现 | `prepare_transfer_tty`，pty 实测 |
| Z-109 | 停滞检测：30 秒无字节移动才放弃，不限制总时长 | 已实现 | `STALL_TIMEOUT`，单测覆盖 |
| Z-110 | 传输结束清空 PTY 残留，防止被当作新会话检测 | 已实现 | `zmodem_finish` + `drain_until_quiet` |
| Z-111 | 失败或取消时向远端发送中止序列（6 字节 CAN + `Ctrl+U` 清行） | 已实现 | `cancel_burst`，单测覆盖 |
| Z-112 | 传输进度显示，写入后主动请求重绘 | 已实现 | `zmodem/progress.rs`、`MuxNotification::PaneOutput` |
| Z-113 | 子进程 stdin/stdout 各用独立单工管道，解析器输入不会回灌给对端 | 已实现 | `make_pipe`，`run()` 双管道 |
| Z-114 | 子进程输出来自管道，对端数据经解析器 sink 单向送入子进程 stdin | 已实现 | `RelayMode::ChildToPty`，单测 `child_to_pty_mode_carries_child_output_only` |
| Z-115 | Ctrl+C 中止传输：取消标志由 UI 与传输线程共享同一 `Arc` | 已实现 | `on_sink_ready` 回传 `ZmodemState::begin` 的 flag |
| Z-116 | 取消时按固定顺序拆解：停 relay → 发 CAN → 消费至连续 1 秒静默 → 丢弃待发输出 → 交还读侧 | 已实现 | `drain_until_quiet`、`discard_pending_output` |
| Z-117 | 本地子进程已死但远端仍在发时，继续吸收并丢弃而非交还普通解析 | 已实现 | `mux/src/lib.rs` `parse_buffered_data` 的 EPIPE 分支 |
| Z-118 | 冷却期过滤 ZHEX 与 ZBIN 两种帧头 | 已实现 | `strip_zmodem_residue`，单测覆盖 |

### 6.2 已知问题

| 编号 | 问题 | 状态 |
|---|---|---|
| Z-902 | 上传失败后的残留字节可能被 ssh 解释为转义序列（如 `~v`），导致会话 Debug 级别被提升 | 已缓解（`sz -e`），未复测 |

**已解决**：早期记录的大文件传输卡死（原 `Z-901`）已定位并修复。根因是单个双向 relay 线程同时承担两个方向：对端停止读取时 socketpair 被写满，relay 阻塞在 `poll(POLLOUT)`，期间不读子进程的 `ZACK`/`ZRPOS`，双向互等，传输停在缓冲区被填满的百分比处。修复为按方向拆分（`Z-114`），并让子进程输入输出走独立管道（`Z-113`）。

---

## 7. SSH 与远程

| 编号 | 需求 | 状态 | 验证 |
|---|---|---|---|
| R-101 | 通过 ssh 连接远程主机的 domain | 已移除（见 N-107） | 原 `mux/src/ssh.rs`、`crates/wezterm-ssh` |
| R-102 | SSH agent 转发 | 已移除（见 N-107） | 原 `mux/src/ssh_agent.rs` |
| R-103 | tmux 控制模式（`tmux -CC`）集成：远端 tmux 的 window/pane 映射为 Faz 的 tab/pane | 已实现（见 7.1） | `mux/src/tmux.rs`、`tmux_commands.rs` |

**tmux 集成的工作机制**：tmux 的 control mode 通过终端转义序列协商。当 pane 输出中出现 `DCS 1000` 请求时，Faz 新建一个 `TmuxDomain` 接管该 pane，把后续输出当作 tmux 控制协议解析。远端发送 `%window-add`、`%layout-change`、`%output` 等控制行，Faz 据此在本地建立对应的 tab 与 pane；本地新建窗口、分屏、套选 pane 时，Faz 反向发送 `new-window`、`split-window`、`select-pane` 命令。效果是：**在 Faz 里可以原生操作远端 tmux 会话**，不需要记住 tmux 前缀键，也不需要嵌套终端。

**窗口模型**：每个 `tmux -CC` 会话独占一个 Faz 窗口（domain 名 `tmux-<pane_id>`，保证多个会话不撞名）。窗口在第一个 tab 落位后才发布给 GUI，因为 `MuxWindowBuilder` 只在 drop 时发 `WindowCreated`。运行 `tmux -CC` 的那个 pane 保留在它原来的窗口里不动。

**新建 tab**：在该窗口里新建 tab 会向 tmux 发 `new-window`，等 `%window-add` 回来后再建立本地 tab，带 10 秒超时避免永久挂起。

**关闭窗口 = detach**：关闭承载 tmux pane 的窗口会走真正的销毁路径（macOS 上其他窗口仍然只是 `order_out()` 隐藏），并通过 `LocalPane::detach_tmux` 向原始 pane 的 pty 写 `detach\n`，等价于 `c-b d`：客户端断开，tmux 会话仍在服务端运行。注意不能用 terminal 的 writer——tmux pane 的 terminal writer 是 `TmuxPtyWriter`，会把内容转成 `send-keys` 发给远端 pane。

**标题归 tmux 所有**：tmux tab 跳过用户 `format-tab-title` 回调（普通与 hover 两条路径都跳过），且忽略内部 shell 的 OSC 2 标题变化，标题始终来自 tmux 的 window 名。

**为什么有用**：长时间 ssh 会话断线后，远端 tmux 里的工作不会丢；通过 control mode 接入后，这些会话在 Faz 里以本地 tab 的形式出现，可以直接用 Faz 的分屏、标签、鼠标选择、滚屏，而不是在单一块终端里看图。

**与 shell 集成中 `TMUX` 变量的区别**：shell 集成里的 `TMUX`/`FAZ_SESSION` 判断的是“用户在 Faz 里手动跑了 tmux”，目的是让 prompt 和 Smart Tab 在那种环境下仍然可用；上面的控制模式集成是另一回事，两者互不依赖。

**普通 `tmux`（非 `-CC`）Faz 感知不到**：ssh 通道里只有字节流，Faz 不解析远端进程。`WEZTERM_IN_TMUX` 这个 user-var 读的是**本地** shell 的 `TMUX`，反映“本地是否在 tmux 里”，不是“远端是否在 tmux 里”。注意不要用它判断远端状态。

### 7.1 控制模式集成的已知缺陷（待评估与修复）

| 编号 | 问题 | 状态 | 定位 |
|---|---|---|---|
| R-901 | 控制模式下 pane 无法输入 | 已修复：`TmuxPty::write` 曾返回 `Ok(0)`，使 `write_all` 以 `WriteZero` 中止 | `mux/src/tmux_pty.rs` |
| R-902 | 关闭 tmux tab 后状态泄漏，后续 `tmux -CC` 无法再拉起新 tab/pane | 未复测 | `DeviceControlMode::Exit` 是主要清理点 |
| R-903 | tmux tab 与本地 tab 在标签栏中无视觉区分 | 未实现 | tab bar 渲染 |
| R-904 | 控制模式提示语 `[This pane is running tmux control mode. Press q to detach]` 与 `q` 键处理 | 保留：`q` 仍用于退出控制模式；正常输入已可用，不再冲突 | `mux/src/localpane.rs` |

**仍待评估**：`move-pane`、`kill-pane` 等反向命令是否完整覆盖；关闭最后一个 tab（而非整个窗口）后的状态清理属于 R-902 的未复测部分。

---

## 8. CLI

| 编号 | 命令 | 状态 |
|---|---|---|
| L-101 | `faz`（无参数）：启动或连接已有实例 | 已实现 |
| L-102 | `faz start` / `faz blocking-start` | 已实现 |
| L-103 | `faz config`：交互式配置 TUI | 已实现 |
| L-104 | `faz init`：安装 shell 集成 | 已实现 |
| L-105 | `faz doctor`：环境与运行时诊断 | 已实现 |
| L-106 | `faz reset`：移除 shell 集成与托管默认值 | 已实现 |
| L-107 | `faz cli`：在指定 pane 中执行命令 | 已实现 |
| L-108 | `faz set-cwd`：同步 pane 工作目录 | 已实现 |

**已移除**：`faz update`（应用内无更新入口）。**更新渠道**：下载新版 DMG 覆盖安装，不提供 Homebrew tap。

---

## 9. macOS 集成

| 编号 | 需求 | 状态 | 验证 |
|---|---|---|---|
| M-101 | 原生菜单栏，五个顶层菜单 | 已实现 | `faz-gui/src/commands.rs` |
| M-102 | 显示/隐藏、最小化、全屏、关闭行为 | 已实现 | Window 菜单 |
| M-103 | 设为默认终端 | 已实现 | Faz 菜单 |
| M-104 | 通知中心集成（首次绘制后初始化，不阻塞启动） | 已实现 | `termwindow/render/paint.rs` |
| M-105 | 最小化 AppleScript 字典（只读属性 + quit） | 已实现 | `docs/features.md` |
| M-106 | 仅构建 Apple Silicon（arm64） | 已实现 | `scripts/build.sh` |

**约束**：
- AppKit 菜单 `keyEquivalent` 会在 `keyDown` 之前拦截按键。`Ctrl+<letter>` 在 TUI 内失效时先查菜单，再查 termwiz。
- 菜单栏初始化时序敏感，改动需带回归测试。

---

## 10. 构建与发布

| 编号 | 需求 | 状态 | 验证 |
|---|---|---|---|
| B-101 | `make fmt` / `fmt-check` / `check` / `test` | 已实现 | `Makefile` |
| B-102 | `make app` 构建可运行的 app bundle | 已实现 | `scripts/build.sh` |
| B-103 | CI：fmt、check、test、日志与提示守卫 | 已实现 | `.github/workflows/checks.yml` |
| B-104 | 发布形态构建（universal / bundle） | 已实现 | `.github/workflows/build-validation.yml` |
| B-105 | 发布说明脚本与格式校验 | 已实现 | `scripts/check_release_notes.sh` |
| B-106 | 配置发布就绪校验 | 已实现 | `scripts/check_config_release_readiness.sh` |

---

## 11. 明确不做

以下能力经产品决策排除，不应重新引入：

| 编号 | 排除项 | 原因 |
|---|---|---|
| N-101 | UI 多语言 / `config.language` | `rust-i18n` 方案已回退，UI 保持英文字面量 |
| N-102 | 应用内更新（菜单、后台检查、`faz update`） | 无更新入口，更新靠下载新 DMG |
| N-103 | Help 菜单与 GitHub 快捷命令 | 已移除 |
| N-104 | AppleScript `do script` | 不向 AppleScript 暴露 shell 执行 |
| N-105 | Remote Files（`sshfs` 远程文件浏览） | 已整体移除，相关依赖一并清除 |
| N-106 | Homebrew tap / cask 分发 | 不维护 tap，更新走 DMG |
| N-107 | 内置 SSH 客户端（`ssh_domains`、SSH agent 转发、`faz cli tlscreds`） | 已整体移除，配置字段一并删除。远程连接用窗格里系统的 `ssh`；tab 标题仍会读取 `WEZTERM_PROG` 和前台进程，为 `ssh` / `mosh` / `autossh` / `et` 显示远端主机名 |
| N-108 | TLS 域（`tls_clients` / `tls_servers`） | 已整体移除。服务端从未接线，`tls_servers` 只被解析不生效；`ClientDomainConfig` 随之压平为 `UnixDomain` |

---

## 附：需求与代码的对应关系

| 功能域 | 主要位置 |
|---|---|
| 终端语义 | `term/`、`termwiz/` |
| 窗口与 pane | `mux/`、`window/` |
| GUI 渲染与输入 | `faz-gui/src/termwindow/` |
| 配置 | `config/`、`lua-api-crates/` |
| ZMODEM | `mux/src/zmodem/` |
| CLI | `faz-cli/` |
| Shell 集成 | `assets/shell-integration/` |
| 构建发布 | `scripts/`、`.github/workflows/` |

---

*本文档随功能变更更新。新增或移除功能时，同步修改对应条目及其状态。*
