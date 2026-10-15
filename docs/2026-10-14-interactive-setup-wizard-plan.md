# 无参数启动的交互式模式选择 / 参数输入界面规划

> 生成日期：2026-10-14
> 状态：**已实现**（2026-10-14，含 15 个新增测试；36 个测试全绿，CLI 行为回归与真实终端端到端验证均已通过）
> 关联文档：[多 Countdown 同屏显示功能规划](./2026-10-03-multi-countdown-plan.md)
> 关联代码：[main.rs](../clock-tui/src/bin/main.rs)、[run.rs](../clock-tui/src/run.rs)、[wizard.rs](../clock-tui/src/app/wizard.rs)、[app.rs](../clock-tui/src/app.rs)、[config.rs](../clock-tui/src/config.rs)

## 1. 背景与目标

当前 `tclock` 是一个纯命令行工具：所有模式与参数都必须通过 clap 子命令给出，`tclock` 裸启动时走 `default.mode`（默认 clock）直接进入时钟界面。想要换模式或改参数，只能退出后重新敲一条命令。

**目标**：新增一个交互式配置界面（下称「向导」），让用户不带任何参数启动 `tclock` 时，可以在终端里完成
「选模式 → 填参数 → 预览 → 运行 → 返回继续切换」的闭环，从而不必记忆子命令和参数拼写。

**非目标**：

- 不替代 CLI。所有既有子命令、全局参数、`--config` 语义与输出行为保持不变。
- 不做配置文件的可视化编辑器（只读展示 `[[countdown.items]]`，不落地改写配置）。
- 不引入新的第三方依赖（见 §8.1 的沙箱约束）。

## 2. 已确认的设计决策

| # | 决策点 | 结论 |
|---|---|---|
| D1 | 触发条件 | **严格零参数**：仅当命令行参数个数为 0（即 `std::env::args_os().len() == 1`）时才进入向导；`tclock -c red`、`tclock --size 3` 等仍按现有 `default.mode` 直接启动 |
| D2 | 界面形态 | **全屏 ratatui 向导**，复用现有渲染栈与 BricksFont |
| D3 | 确认之后 | 启动所选模式；在该模式内按 `q` **返回向导**（而非退出进程），从向导再退出才结束进程 |
| D4 | 首期范围 | 四个模式全部纳入（Clock / Timer / Stopwatch / Countdown）+ 全局字段（color、size、config 路径） |
| D5 | 附加能力 | 含**实时预览**窗格（用真实 widget 渲染当前参数） |

## 3. 启动流程与触发判定

### 3.1 判定规则

```rust
// bin/main.rs —— clap 照常解析（保证 --help/--version 行为不变），再做零参数判定
let app = App::parse();
let bare_invocation = std::env::args_os().len() == 1;   // argv[0] 之外没有任何参数
```

- `app.mode.is_none()` **不足以**作为判据：`tclock -c red` 同样 `mode == None`，但那不是「裸启动」。因此必须以 `argv` 长度为准（D1）。
- `tclock --help` / `-V` 在 clap 内部就已打印并退出，不受影响。
- 判定要在 `enable_raw_mode()` / `EnterAlternateScreen` 之前完成，保证 `--help` 与错误信息不会被写进备用屏滚走。

### 3.2 顶层流程

```
tclock                                    tclock timer -d 25m
   │                                          │
   ├─ argv 长度 > 1 ──► 现有路径（App::init_app + 运行循环）──► 退出
   │
   └─ argv 长度 == 1
        └─► Wizard::ModeSelect ──选中模式──► Wizard::ParamForm
                 ▲                                 │  Enter 运行
                 │  q / Esc（退出进程）              ▼
                 └────────── q ────────────── App 运行循环 ──► on_exit 打印
                   （c/w/t 也可直接跳回并预选中对应模式，见 D6）
```

## 4. 向导架构

### 4.1 模块归属（**必须在 lib 内**）

`Clock`、`Timer::new`、`Stopwatch::new`、`DurationFormat` 均为 `pub(crate)`（见
[app/modes/timer.rs:33](../clock-tui/src/app/modes/timer.rs#L33)、
[stopwatch.rs:18](../clock-tui/src/app/modes/stopwatch.rs#L18)、
[modes.rs:26](../clock-tui/src/app/modes.rs#L26)），**二进制 crate 无法直接构造它们**。
因此向导与运行循环必须放在 `clock_tui_ya` 库内，`bin/main.rs` 只保留入口决策。

### 4.2 计划新增 / 改动的文件

| 文件 | 动作 | 说明 |
|---|---|---|
| `clock-tui/src/run.rs` | **新增** | 公开入口 `run(app: App) -> Result<(), Box<dyn Error>>`；备用屏与 raw mode 的唯一进出点；模式运行循环与按键分发 |
| `clock-tui/src/app/wizard.rs` | **新增** | 向导状态机、模式列表、表单模型、布局与渲染、校验 |
| `clock-tui/src/app.rs` | 改动 | 新增 `AppParams` 与 `ModeConfig`；`App` 增加 `params` 字段；`init_app()` 改为逐字段来源优先级；`parse_duration` / `parse_color` / `parse_datetime` / `parse_timezone` 提升为 `pub(crate)` 供向导复用 |
| `clock-tui/src/lib.rs` | 改动 | `pub mod run;` |
| `clock-tui/src/bin/main.rs` | 改动 | 收敛为「解析 → 零参数判定 → 构造 App → `run(app)`」约 20 行 |

> 命名待定：`run.rs` 也可命名为 `session.rs` / `runner.rs`，实现前定一个即可。

### 4.3 状态机

```rust
pub(crate) enum WizardState {
    ModeSelect,                    // 模式列表
    ParamForm { mode: FormKind },  // 参数表单（含实时预览）
    ConfirmDiscard { next: Box<WizardState> }, // 有未保存修改时离开的二次确认
}

pub(crate) enum WizardOutcome {
    Run,        // 提交：带着已解析的参数去运行
    Quit,       // 真正退出进程
}

pub(crate) struct Wizard {
    state: WizardState,
    form: Form,          // 当前模式的字段集合 + 焦点 + 未保存标记
    source: ParamSource, // CLI 值 / 配置文件值，用于表单初始化与「来源」标注
    config_path: Option<PathBuf>,
}
```

事件入口统一为 `Wizard::on_key(KeyCode) -> WizardOutcome`，`Wizard::ui(&self, f: &mut Frame)` 负责渲染。
运行循环与向导循环共用一个 `ratatui::Terminal`（见 §8.3）。

### 4.4 表单字段模型（通用，便于校验与测试）

不按模式硬编码渲染逻辑，而是把每个模式的参数描述为一组**字段**，交给统一渲染器：

```rust
enum FieldKind {
    Choice { options: Vec<&'static str>, selected: usize }, // 时区（含「本地」）、颜色
    Toggle { on: bool },                                    // show_date / show_seconds / millis …
    Text { value: String, placeholder: &'static str },      // 标题、单次 time、config 路径
    Int  { value: u16, min: u16, max: u16 },                // size
    TimeList  { items: Vec<String> },                       // timer 的 durations
    TitleList { items: Vec<String> },                       // timer 的 titles
    CommandList { items: Vec<String> },                     // timer 的 execute
}
struct Field { label: &'static str, kind: FieldKind, error: Option<String>, visible: bool }
```

好处：字段渲染、Tab 焦点流转、内联错误提示、上下移动只写一次；每个模式只负责声明字段 + 提供校验函数。

## 5. 界面设计

### 5.1 模式选择（ModeSelect）

```
┌ tclock · 选择模式 ───────────────────────────────────────────────┐
│                                                                  │
│   ▸ Clock        显示当前时间，支持时区                          │
│     Timer        倒计时 / 番茄钟，可多段顺序执行                 │
│     Stopwatch    秒表                                           │
│     Countdown    倒数日（单个或配置文件的网格）                  │
│                                                                  │
│   全局：color=green  size=1  config=(未指定)                     │
├──────────────────────────────────────────────────────────────────┤
│ ↑/↓ 选择   Enter 进入参数   q/Esc 退出                           │
└──────────────────────────────────────────────────────────────────┘
```

- 列表项右侧显示一行说明；底部状态栏显示当前全局参数与（若指定了）config 路径。
- 已给 `--config` 时不进向导（D1），所以这里的 config 初始值恒为「未指定」，只有用户在表单里选了文件才会变。

### 5.2 参数表单（ParamForm）

宽屏（≥ 100 列）左右分栏，窄屏（60 ~ 99 列）只看表单：

```
┌ Timer 参数 ─────────────────────┬ 预览 ────────────────────────┐
│ 时长（至少 1 项）               │                              │
│   ▸ 25m                    [Del]│         25:00                │
│     5m                    [ ↑↓ ]│        Work                  │
│   + 添加 (a)                    │                              │
│ 标题                            │                              │
│   ▸ Work                        │                              │
│   + 添加 (a)                    │                              │
│ 重复        [ 关 ]  ←/→         │                              │
│ 暂停启动    [ 关 ]              │                              │
│ 显示毫秒    [ 开 ]              │                              │
│ 结束自动退出[ 关 ]              │                              │
│ 结束执行命令                    │                              │
│   （空）                 + 添加 │                              │
├─────────────────────────────────┴──────────────────────────────┤
│ Tab/↑↓ 移动  Enter 编辑  Space 切换  a 添加  d 删除  Enter(末) 运行 │
│                                                          Esc 返回 │
└────────────────────────────────────────────────────────────────┘
```

- 预览窗格 = 真实的 widget（`Clock` / `Timer` / `Stopwatch` / `Countdown`）渲染到子 `Rect`；多 countdown 时用现有的 `render_countdown_grid` 复用网格逻辑。
- 校验失败时在对应字段右侧显示内联错误（如 `time 格式无效，例：20:00 / 2026-01-01`），并禁用「运行」。
- 终端过小（< 50 列 或 < 15 行）时整屏显示「终端窗口过小（至少 50x15）」并等待 resize，不 panic。

### 5.3 键盘映射

| 上下文 | 按键 | 行为 |
|---|---|---|
| 列表 | ↑/↓ 或 k/j | 移动选择（j/k 需确认不冲突，首期可只做方向键） |
| 列表 | Enter | 进入参数表单（用 CLI/配置文件值预填） |
| 列表 | q / Esc / Ctrl+C | 退出进程 |
| 表单 | Tab / Shift+Tab | 下一个 / 上一个字段（跳过不可见字段） |
| 表单 | ↑/↓ | 同 Tab；在列表类字段内移动当前项 |
| 表单 | Enter | 编辑当前字段；在最后一个字段上 = 提交运行 |
| 表单 | Space / ←/→ | 切换布尔与枚举 |
| 表单 | a / d | 列表类字段：追加一项 / 删除当前项（不含最后一项时给出提示） |
| 表单 | Esc | 有修改 → `ConfirmDiscard`；无修改 → 返回列表 |
| 表单 | Ctrl+R | 直接以当前参数运行 |
| 运行中 | q | 返回选择界面（调用 `App::on_exit()` 后重入备用屏） |
| 运行中 | Esc | 同 `q` |
| 运行中 | Ctrl+C | 直接退出进程 |
| 运行中 | c / w / t | **待定（D6）**：跳回向导并预选中对应模式，还是保留现有「原地换模式」语义 |

## 6. 参数与校验规格

### 6.1 各模式字段

| 模式 | 字段 | 控件 | 映射到的 `Mode` 字段 |
|---|---|---|---|
| **Clock** | 时区 | Choice（本地 + 常用时区列表 + 手输，用 `parse_timezone` 校验） | `timezone` |
| | 显示日期 / 显示秒 / 显示毫秒 | Toggle ×3 | `no_date`(取反) / `no_seconds`(取反) / `millis` |
| **Timer** | 时长列表（≥1） | TimeList，`parse_duration` 校验 | `durations` |
| | 标题列表 | TitleList | `titles` |
| | 重复 / 暂停启动 / 显示毫秒 / 结束自动退出 | Toggle ×4 | `repeat` / `paused` / `no_millis`(取反) / `auto_quit` |
| | 结束执行命令 | CommandList | `execute` |
| **Stopwatch** | — | 无字段，直接运行 | `Mode::Stopwatch` |
| **Countdown** | 来源 | Choice：`单个时间` / `配置文件 __countdown.items__` | 决定下面两组字段的可见性 |
| | （单个时间）目标时间 | Text，`parse_datetime` 校验 | `time` |
| | （单个时间）标题 | Text | `title` |
| | 继续过零计时 / 反向计时 / 显示毫秒 | Toggle ×3 | `continue_on_zero` / `reverse` / `millis` |
| | （配置文件）条目只读列表 | 展示 `title` + `time` + 剩余量，只读（首期） | 走现有 `build_countdown_slots` |
| **全局** | 颜色 | Choice（16 色名 + `#RRGGBB` 手输），`parse_color` 校验 | `color` |
| | 字号 | Int（1..=10） | `size` |
| | config 路径 | Text + 「载入」动作，校验文件存在且 TOML 可解析 | `config` |

### 6.2 校验规则（必须内联报错并阻止运行）

| 编号 | 规则 | 原因 |
|---|---|---|
| V1 | Timer 的 durations **至少 1 项且全部可解析** | `Timer::remaining_time()` 会执行 `self.durations[idx]`，空列表 **panic**（[timer.rs:68](../clock-tui/src/app/modes/timer.rs#L68)） |
| V2 | Countdown 选了「单个时间」时 time 必填且可解析 | 否则运行后只会打印警告、黑屏（现有 main.rs 的校验逻辑前移到这里） |
| V3 | Countdown 选了「配置文件」时，config 必须存在、可解析，且含 `[[countdown.items]]` 或 `[countdown].time` | 同 V2 |
| V4 | 时区必须是合法 IANA 名（小写会被拒绝） | `Tz::from_str` 大小写敏感 |
| V5 | 颜色必须是 16 色名之一或 `#RRGGBB` | `parse_color` |
| V6 | size ≥ 1 | 现有 `--size` 语义 |
| V7 | execute 每项非空；含空格时按现有语义**整条拼接**后交给 `sh -c`，界面需提示这一点 | 避免用户误以为每个元素是独立参数（Windows 上无 `sh`，首期仅提示） |

## 7. 参数回流：`AppParams` / `ModeConfig`

这是实现上最关键的一处结构调整。

### 7.1 问题

现在 `App::init_app()` 把「CLI 值」与「配置文件值」用 `||` / `unwrap_or` 混在一起合并，例如
`*repeat || timer_config.map(|c| c.repeat).unwrap_or(false)`（[app.rs:261](../clock-tui/src/app.rs#L261)）。
bool 上无法区分「未指定」与「显式 false」，所以向导解析出的 `show_millis = false` 会被配置文件的
`show_millis = true` 覆盖——**向导的选择会静默失效**。

### 7.2 方案：独立于 `Mode` 的「已解析参数」

不让向导去构造 `Mode`（`Option<Tz>`、`Option<DateTime>` 这些字段无法表达「向导已给出」），
而是新增一个参数结构，与 `Config` 同形但全部可选：

```rust
/// 向导/运行循环产出的、已逐字段解析好的参数覆盖层
#[derive(Debug, Clone, Default)]
pub struct AppParams {
    pub color: Option<Color>,
    pub size: Option<u16>,
    pub config: Option<PathBuf>,

    // clock
    pub timezone: Option<Option<Tz>>,   // Some(None) = 明确选择「本地时区」
    pub show_date: Option<bool>,
    pub show_seconds: Option<bool>,
    // timer
    pub durations: Option<Vec<Duration>>,
    pub titles: Option<Vec<String>>,
    pub repeat: Option<bool>,
    pub start_paused: Option<bool>,
    pub auto_quit: Option<bool>,
    pub execute: Option<Vec<String>>,
    pub show_millis: Option<bool>,
    // countdown
    pub countdown_time: Option<DateTime<Local>>,
    pub countdown_title: Option<Option<String>>,
    pub continue_on_zero: Option<bool>,
    pub reverse: Option<bool>,
    pub use_config_items: Option<bool>, // true = 走 [[countdown.items]] 网格
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ModeKind { Clock, Timer, Stopwatch, Countdown }

/// 向导产出的完整描述
#[derive(Debug, Clone, Default)]
pub struct ModeConfig {
    pub kind: ModeKind,
    pub params: AppParams,
}
```

### 7.3 `init_app()` 改造

`App` 增加字段 `#[clap(skip)] pub params: Option<AppParams>`；`init_app()` 内先计算一份**逐字段解析**的本地配置，再据此构造 widget：

```rust
let file = self.config.as_deref().and_then(Config::load_from);   // CLI --config
let p    = self.params.take();                                   // 向导最高优先级

let cfg = resolve(&app_meta, file.as_ref(), p.as_ref());         // color / size / config 路径
// cfg.clock / cfg.timer / cfg.countdown 同理由 resolve 逐字段求出
```

字段优先级统一为：**向导（params）> CLI 显式值 > 配置文件 > 硬编码默认**。因为 `params` 里的每个
`Option` 都代表「向导明确指定」，所以显式的 `false` 不会被低优先级来源翻回 `true`。

CLI 与向导可以同时存在（例如运行中按 `c` 跳回向导，此时 `self.color` 仍保留 CLI 传入的值），
所以不是二选一，而是分层。

### 7.4 运行中按键切换的相应改动

[main.rs:63-78](../clock-tui/src/bin/main.rs#L63-L78) 里 `c` / `w` / `t` 直接构造 `Mode` 字面量。改造后这些处理要移入库内，并改为：

```rust
mode_config = Some(ModeConfig { kind: ModeKind::Clock, params: AppParams::clock_from_cli(&app) });
app.apply_mode_config(mode_config)?;   // 重新 init_app 并保留 CLI 值
```

这样「原地切换」与「向导切换」走同一条代码路径，不再有两套构造逻辑。

## 8. 实现要点

### 8.1 依赖约束（重要）

沙箱内 TLS 不可用，cargo 无法联网拉新 crate（见 [AGENTS.md](../AGENTS.md)）。首期**只允许使用现有依赖**：
`ratatui 0.25` + `crossterm 0.25` + `clap 4` + `chrono` + `chrono-tz` + `regex` + `toml` + `serde`。

由此带来的两个限制：

1. **不做文本粘贴**。`crossterm 0.25` 没有 `Event::Paste`（该事件在 0.28 引入），路径/标题只能逐字符输入；长路径场景建议提示用户改用 `--config`。
2. **不做下拉浮层**。`ratatui 0.25` 无状态化弹层组件，时区/颜色用「枚举轮转 + 手输模式」实现。

如确需新增依赖，必须先在**沙箱外**执行 `cargo fetch --target x86_64-pc-windows-gnu`，再回沙箱内 `--offline` 构建。

### 8.2 终端进出与 `on_exit` 时序

模式运行结束（按 `q`）返回向导时：

```
disable_raw_mode() → LeaveAlternateScreen → app.on_exit()（打印秒表时间等）→ flush
   → 再次 EnterAlternateScreen → 重绘向导
```

`App::on_exit()` 的语义是「离开备用屏后输出」，**不能在备用屏内调用**，否则秒表成绩会被随后的清屏吞掉。
这一段顺序建议在 `run.rs` 里用单独的 helper 固定下来，避免后续改动打乱。

### 8.3 单终端复用

向导与模式共用同一个 `Terminal<CrosstermBackend<Stdout>>`，不要为向导再建一个（重复 `EnterAlternateScreen`
会造成光标/屏体状态不一致）。运行循环建议统一为「`WizardState::Running` 也由一个事件循环驱动」，
按键分发按状态路由，避免两个循环各自持有 `App` 造成借用与状态同步问题。

## 9. 测试计划

| 编号 | 测试 | 做法 |
|---|---|---|
| T1 | 零参数判定 | 把 `argv` 判定抽成纯函数 `is_bare_invocation(len: usize) -> bool` 或 `fn bare(args: &[OsString]) -> bool`，断言 `len == 1` 为真、`2` 为假 |
| T2 | 向导首屏可渲染 | `TestBackend` 渲染 ModeSelect，断言输出含 `Clock` / `Timer` / `Stopwatch` / `Countdown` |
| T3 | 小终端不 panic | `TestBackend::new(40, 10)` 渲染 ModeSelect 与各表单，断言含「终端窗口过小」 |
| T4 | 状态机流转 | 合成 `KeyCode` 序列：列表 → 表单 → 运行；以及 `Esc` → `ConfirmDiscard` |
| T5 | 校验规则 | 每个 V1~V7 各一条：`form.validate()` 返回对应错误，合法输入返回 `Ok` |
| T6 | 表单渲染 | 每个模式的表单在 120x40 下渲染，断言含关键标签与预览数字块（`█`） |
| T7 | 参数回流优先级（核心） | 构造 `App { params: Some(AppParams { show_millis: Some(false), .. }), config: Some(指向 `examples/config.toml`) }`，断言 `Timer` 仍为 `HourMinSec`，不被文件里的 `show_millis = true` 覆盖 |
| T8 | 既有 CLI 解析回归 | 保留对 `App::parse_from(["tclock","timer","-d","25m"])` 等既有用法的断言 |
| T9 | 运行中按键路径 | 断言 `c`/`w`/`t` 走的 `ModeConfig` 构造结果与改造前 `Mode` 字面量语义一致 |

> 注意：不要用 `std::env::set_current_dir` 来测配置路径（进程级全局状态，多测试并行会互相干扰），
> 改为直接测 `Config::load_from(显式路径)` 与「表单校验函数」。

## 10. 文档与示例更新

- [README.md](../README.md)：说明 `tclock` 裸启动进入向导；现有 `tclock clock` / `timer -d 5m` 等用法不变。
- [README_ORI.md](../README_ORI.md)：上游文档整体保留，仅在其顶部加一行「本 fork 中 `tclock` 裸启动行为见 README.md」的注释，不改动正文。
- 可选：`assets/` 补一张向导截图（与 `screenshot-multi-countdown.png` 并列）。

## 11. 建议的实施顺序

| 阶段 | 内容 | 产出 |
|---|---|---|
| P0 | `AppParams` / `ModeConfig` / `init_app()` 分层改造 + T7/T8 | 纯重构，无界面，先保证参数优先级正确 |
| P1 | `run.rs`：备用屏进出、模式循环、`on_exit` 时序；`bin/main.rs` 收敛 | 零参数仍未接向导，行为与现状等价 |
| P2 | 向导骨架：状态机 + 模式列表 + 小终端兜底 | T1~T4 通过，可跑通「选模式 → 用默认参数运行 → 返回」 |
| P3 | 参数表单 + 校验（先 Clock / Stopwatch，再 Timer，最后 Countdown） | T5/T6 通过 |
| P4 | 实时预览窗格 | D5 落地 |
| P5 | 文档、示例、截图；全量 `cargo test --offline` 回归 | 收尾 |

## 12. 待确认的决策点（实现时的最终结论）

| 编号 | 问题 | 最终结论 |
|---|---|---|
| **D6** | 运行中按 `c` / `w` / `t` 的语义 | **向导会话**：跳回向导并预选中对应模式（表单内容保留）；**CLI 会话**：保持原有「原地换模式」语义不变（经 `ModeConfig::preset` 走同一条 `apply_mode_config` 路径） |
| **D7** | 表单「运行」的确认方式 | **`Ctrl+R` 显式运行**（列表与表单内均可），Enter 只做编辑/确认输入 |
| **D8** | 是否在退出时记住上次参数 | **不做落盘**；进程内表单按模式缓存（`Wizard.forms`），切换模式不丢编辑 |
| **D9** | 向导里能否编辑 `[[countdown.items]]` | **首期只读**：预览窗格直接渲染网格，表单只显示条目数，不回写配置 |
| **D10** | Timer 的 `titles` 与 `durations` 数量不一致时 | 保持现有 `titles[min(idx, len-1)]` 语义，未改 |

### 实现后记（与规划正文不同的地方）

1. **顺带修了一个存量崩溃**：原 `t` 快捷键构造的 `Mode::Timer` 带 `durations: vec![]`，`Timer::remaining_time()` 首帧即越界 panic。现在参数解析层对空 durations 兜底为 CLI 默认值 `[5m]`（有测试锁定）。
2. **`build_countdown_slots` 拆出静默变体**：预览每帧重建 widget，原实现的 `eprintln!` 警告会污染备用屏；新增 `build_countdown_slots_silent` 供预览使用，`Config::load_from_quiet` 同理。
3. **CJK 宽字符的两个实测坑**：① ratatui 0.25 把宽字符的续格置为空格，测试断言缓冲文本前需先去掉空格；② conhost 把 `↑↓←→` 视为双宽，会造成后续字符被覆盖，UI 文案改用「上下键/左右键」。
4. **Esc 在运行中 = q**（返回向导/退出），CLI 会话同样生效；Ctrl+C 在任何状态直接退出进程（raw mode 下原本 Ctrl+C 不产生 SIGINT，这是新增能力）。
5. **（2026-10-14 追加）Countdown 拆为两个入口**：单个时间与 `[[countdown.items]]` 网格的参数本质互斥，模式列表现为 5 项（`ModeKind::CountdownGrid` 新增，仍映射到同一个 clap `Mode::Countdown` 变体）；`CdSource` 选择字段与 `FieldKind::Choice` 随之移除。顺带消除了 grid 分支在「向导选了网格但配置文件恰好加载失败」时 `unwrap` 的 panic 隐患（退化为警告路径）。
6. **（2026-10-14 追加）向导状态落盘**：`WizardState` 写入 exe 同目录的 `tclock-wizard.toml`，记录上次成功载入的 config 路径（持久化时转为绝对路径），下次启动预填并自动预载；清空该字段则删除状态文件。

## 13. 验收清单

- [ ] `tclock` 裸启动进入向导；`tclock -c red`、`tclock timer -d 25m`、`tclock --help` 等行为与改造前完全一致
- [ ] 四个模式都能在向导里配置并成功启动；Stopwatch 无字段可直接运行
- [ ] 每个模式的参数都能被实时预览，且预览与运行结果一致
- [ ] V1~V7 校验全部内联报错，且错误状态下无法提交
- [ ] 运行中按 `q` 能稳定返回向导；秒表成绩在返回前正确打印且不被吞掉
- [ ] 终端小于 50x15 时给出提示而非 panic；resize 后自动恢复
- [ ] `cargo build --offline` / `cargo test --offline` 全绿，既有 10 个 countdown 测试无回归
- [ ] 未引入任何新依赖

## 14. 风险

| 风险 | 说明 | 缓解 |
|---|---|---|
| 空 durations panic | `Timer::remaining_time()` 对空列表会越界 panic（[timer.rs:68](../clock-tui/src/app/modes/timer.rs#L68)） | V1 校验硬性拦截；预览渲染前先校验，禁止构造空 durations 的 Timer |
| `init_app` 改造面大 | 该函数同时承担默认模式推导、颜色/字号回退、widget 构造，改动易引入静默回归 | P0 单独成阶段，先把 T7/T8 写出来再改；逐字段对照现有 `unwrap_or` 语义 |
| 两套事件循环 | 向导与模式各持一个循环会让 `App` 状态与借用关系变复杂 | §8.3 单终端 + 按状态路由按键 |
| 快捷键行为变更 | `c`/`w`/`t` 的语义（D6）会变化，且当前未写入文档 | 在 README 中补全快捷键表；如担心兼容，可先保留原地切换、把入口留给向导 |
| 中文/宽字符对齐 | 表单里输入中文 title 时，`Span` 宽度按字节估算会错位 | 首期主要输入 ASCII（时间/时长/路径）；标题类字段容忍轻微错位，必要时用 `unicode-width`（需沙箱外拉依赖） |
