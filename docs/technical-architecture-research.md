# Linger 跨平台技术架构调研

调研日期：2026-11-10。面向 macOS 开发、Windows 日常使用、两端均支持的一期需求。依据官方文档与上游源码，未安装运行这些方案，未实测资源占用或双平台兼容性。功能与参数沿用 [一期共识](phase-1-scope.md)。

## 推荐方向

优先验证 **Tauri 2 + React/TypeScript 界面 + Rust 本地核心 + SQLite**。截图、录音、播放、按键、定时器、会话及记忆由本地核心拥有，关闭界面不结束陪伴。Electron + TypeScript 是媒体生态更现成的备选。

已按此方案建立首版实现并完成 macOS 构建与启动检查；Windows 和真实云端／硬件链路仍待验证。实现范围见 [README](../README.md)。理由是一期无虚拟形象、按键录音、分钟级单图观察，适合以本地后台应用为中心。不能仅凭安装包大小推断长期 CPU、内存或稳定性；Rust 也不会直接减少模型 token 费用。

后续已明确的一期约束：所有模型使用云端 API，以 OpenAI 官方为开发与兼容基准；默认直连官方，允许配置局域网 NewAPI 中转，不运行本地模型。不做账号系统，用户自带 Key。下文关于多厂商或本地推理的内容仅保留为调研背景，不属于一期实现要求。

## 现有项目的架构

| 项目 | 核查到的架构 | 适合参考的部分 |
| --- | --- | --- |
| N.E.K.O | 当前参考仓库为 Python 会话、记忆和 Agent 服务，Web 前端与外部桌面壳协作 | 主动陪伴、截图轮次归属、工具与语音限制；产品范围明显大于一期 |
| AIRI | Electron + Vue/TypeScript，共享音频、模型及屏幕采集包 | UI 无关的音频生命周期、结束采集与取消区分、播放组停止 |
| Open-LLM-VTuber | Electron + React/TypeScript，Python FastAPI/WebSocket 后端 | ASR → 文本与图片 → Agent → TTS 的分层；若大量采用 Python 本地模型更有价值 |
| Amica | Next/React + Tauri 1.x 薄壳 | Web UI 与桌面壳拆分；所查入口不能证明其已解决全局 PTT 与后台读屏 |
| Handy | Tauri 2 + React/TypeScript + Rust，支持跨平台按住录音、松开转写 | 与我们最相关的 PTT、音频设备管理、托盘和原生模块拆分；它本身不是陪伴应用 |
| Screenpipe | Tauri UI、Rust 采集与存储引擎、SQLite | 屏幕与音频后台能力的组织方式；连续录制、OCR 和完整历史规模超出一期所需 |

主源：[AIRI 桌面依赖](https://github.com/moeru-ai/airi/blob/main/apps/stage-tamagotchi/package.json)、[AIRI 音频管线](https://github.com/moeru-ai/airi/blob/main/packages/pipelines-audio/README.md)、[Open-LLM-VTuber 会话实现](https://github.com/Open-LLM-VTuber/Open-LLM-VTuber/blob/main/src/open_llm_vtuber/conversations/single_conversation.py)、[其前端依赖](https://github.com/Open-LLM-VTuber/Open-LLM-VTuber-Web/blob/main/package.json)、[Amica Rust 入口](https://github.com/semperai/amica/blob/master/src-tauri/src/main.rs)、[Amica Cargo](https://github.com/semperai/amica/blob/master/src-tauri/Cargo.toml)、[Handy README](https://github.com/cjpais/Handy/blob/main/README.md)、[Handy Cargo](https://github.com/cjpais/Handy/blob/main/src-tauri/Cargo.toml)、[Screenpipe 架构](https://screenpipe.com/security/architecture)。

Screenpipe 当前仓库采用 source-available 商业许可，不应将其当作可无条件复用的开源底座；本方案只参考架构。[上游说明](https://github.com/screenpipe/screenpipe/blob/main/README.md)

## 候选桌面技术

| 方案 | 优点 | 本项目的主要代价 | 建议 |
| --- | --- | --- | --- |
| Tauri 2 + Rust | 原生核心适合常驻；官方快捷键有按下和松开事件；可直接管理数据库与设备 | 系统 WebView 不同；应验证原生截图、录播、权限与发布 | 优先做双平台样机 |
| Electron + TypeScript | Chromium 媒体链路、屏幕采集和 Node SDK 生态现成 | 官方全局快捷键不提供松键事件，需要 native hook；媒体宿主与后台生命周期需管理 | 强备选 |
| Flutter + Dart | 原生桌面 UI、支持两平台，可用 FFI 和插件 | hotkey_manager 当前 Windows 只有全局按下事件；音频插件两端能力也不完全相同 | 可行，但不会因使用 Flutter 自动解决系统能力 |
| Avalonia + C# | Win/macOS 桌面支持，SharpHook 可提供全局按键事件 | 音频、屏幕采集还需额外库与原生适配 | 有竞争力，但本需求没有明显独占优势 |
| Qt/PySide6 | Qt Multimedia 有屏幕、窗口、音频采集；便于接 Python 模型 | 多运行时打包，全局键仍需处理；采集帧率需控制 | 重度本地 Python 模型时重新评估 |
| Wails + Go | 系统 WebView、Go 后台、托盘与全局快捷键 | 官方快捷键接口仅按下回调；仍需补媒体与原生输入 | 可行，当前无明显优先理由 |

关键证据：

- [Electron globalShortcut](https://www.electronjs.org/docs/latest/api/global-shortcut/) 只有按下回调；[Tauri ShortcutEvent](https://v2.tauri.app/reference/javascript/global-shortcut/) 有 Pressed/Released。Tauri 的接口存在不代表所有组合键均可靠，仍需验证修饰键松开顺序、冲突、长按和锁屏。
- [Electron desktopCapturer](https://www.electronjs.org/docs/latest/api/desktop-capturer) 提供屏幕和窗口来源；默认缩略图不适合直接做文字识别图。分钟级请求不应维持不必要的高帧率采集流。
- [Tauri WebView](https://v2.tauri.app/reference/webview-versions/) 在 Windows 使用 WebView2、macOS 使用 WKWebView。因此推荐将关键音频链路放原生层，而不是假定 Web API 两端表现一致。
- [Flutter hotkey_manager](https://github.com/leanflutter/hotkey_manager) 明确全局 key-up 当前仅 macOS；[record](https://github.com/llfbandit/record/blob/master/record/README.md) 列出两端录音与处理能力差异。
- [Avalonia macOS](https://docs.avaloniaui.net/docs/platform-specific-guides/macos)、[SharpHook](https://github.com/TolikPylypchuk/SharpHook/blob/main/README.md)；[Qt 窗口采集](https://doc.qt.io/qt-6/qwindowcapture.html)、[屏幕采集](https://doc.qt.io/qt-6/qscreencapture.html)；[Wails FAQ](https://wails.io/faq/)、[全局快捷键](https://wails.io/features/keyboard/global-shortcuts/)。

## 建议的内部职责

UI 通过 Tauri commands/events 与 Rust 核心通信，一期无需为此增加本地 HTTP 服务或 Node/Python sidecar。Tauri 自身仍有 Core 与系统 WebView 进程，不把“一个安装应用”描述成单进程。[进程模型](https://v2.tauri.app/concept/process-model/)

| 模块 | 职责与候选实现 |
| --- | --- |
| 桌面与界面 | Tauri 2；React + TypeScript + Vite；聊天、悬浮状态条、设置、记忆管理 |
| 会话协调器 | Rust 异步任务；统一轮次 ID、用户优先级、取消、迟到事件过滤 |
| 截图服务 | 先验证 XCap 的屏幕／窗口单图采集；区域裁剪、缩放、去重；必要时补平台实现 |
| 按键与音频 | 官方 shortcut 插件或 Handy 的热键实现作为候选；CPAL／handy-recorder 采集，原生播放；设备切换、重采样、缓冲和立即停播 |
| 模型适配器 | Rust HTTP/SSE/WebSocket；默认连接 OpenAI 官方，可配置 Base URL 接局域网 NewAPI；按 ASR、对话／视觉、摘要、语音输出、向量化分用途配置；封装不同 API 的事件与取消语义 |
| 主动调度 | 核心进程拥有定时器，窗口隐藏仍工作；复用一期上下文预算和画面去重规则 |
| 记忆与存储 | SQLite；单一写入入口、增量摘要、明确偏好即时写入、历史检索 |

候选库：[XCap](https://github.com/nashaofu/xcap)、[CPAL](https://github.com/RustAudio/cpal)、[handy-recorder](https://github.com/handy-computer/handy-recorder)、[Handy 热键桥](https://github.com/cjpais/Handy/blob/main/src-tauri/src/shortcut/handy_keys.rs)。Handy 的模块可作为参考和待验证依赖，不能因已有项目使用就承诺全部硬件兼容。CPAL 是音频 I/O，不自带完整通话回声消除；按键先停播能降低双工复杂度，但仍需测试外放、蓝牙和其他应用的声音。

不把所有原始 PCM 或完整截图反复通过 JSON 转给 UI；核心直接调用服务商，界面只接收状态、转写、回答和需要显示的预览。工具截图必须实际进入模型可用的图像消息通道。

## 语音与 Agent 框架取舍

LiveKit 提供 Python/Node Agent、WebRTC 房间和 STT/LLM/TTS 管线，Pipecat 提供 Python 多模态音频管线及多种传输方式。它们对于云端实时通话、多客户端、自然双工很有价值。[LiveKit](https://docs.livekit.io/agents/)、[Pipecat](https://github.com/pipecat-ai/pipecat)、[Pipecat 传输](https://docs.pipecat.ai/client/concepts/choosing-a-transport)

一期是本地 PTT 和分钟级截图，优先用小型会话状态机与独立服务适配器。暂不引入房间服务器、额外 Python 语音运行时或多 Agent 编排；未来增加远程实时通话或依赖 Python 的本地推理时再评估。这里是基于范围的工程选择，不代表这些框架无法实现一期。

## 云端模型与可配置中转地址

- 连接设置提供全局 Base URL（默认 `https://api.openai.com/v1`）与用户自带 API Key，各用途共用连接配置并独立设置模型 ID。Key 存系统凭据存储；允许显式设置局域网 HTTP 地址。开发使用官方服务，中转单独验证 Responses、流式事件、图片、工具、搜索、ASR 上传及 TTS 参数透传；不预设 NewAPI 完全兼容，不静默降级或切换服务。
- 本地不包含模型文件下载、GPU 推理、Whisper／ONNX 等本地模型运行时，也不需要 Python 推理进程。原生录播、音频格式转换和图像缩放仍保留。
- 对话、视觉和工具以 Responses API 为优先接入路径；会话内容和上下文预算仍由本地核心控制。截图和记忆工具在本地执行，网络搜索可使用支持模型的官方 web search 工具。模型可选范围按任务能力和账户可用性验证。
- PTT 的兼容基线是松键后上传完整录音进行转写；若需要边录边识别，另外接 Realtime transcription，不能把文件转写的流式文字输出视为麦克风流式上行。
- 语音输出独立封装。用户已决定一期暂定 `gpt-4o-mini-tts`，使用 `/v1/audio/speech`，不提前引入 Realtime 输出或迁移实现。支持配置 `voice`、`instructions`、`speed`（0.25–4.0，默认 1.0）、`response_format`，提供试听；本地音量独立设置。采用简单分段合成、顺序播放与按键取消。2026-11-10 核查时官方说明该模型将于 2027-01-06 停用，保留此信息作为后续迁移依据，不改变一期选择。
- 云端向量化不等于云端数据库。需要语义检索时，对新增或修改的长期条目／历史摘要批量生成向量，向量与原文仍存本地；不对每次观察或全部重复上下文重新计算。索引记录模型、维度与内容版本，变更模型时重建对应索引，失败时可继续关键词检索。
- 采用统一的超时、有限重试和轮次取消机制。网络恢复后不补播积压的主动发言；记忆写入只有成功后才确认。保留此前“不做每日预算自动降频”的决定。

依据：[Responses 迁移与工具](https://developers.openai.com/api/docs/guides/migrate-to-responses)、[文件转写](https://developers.openai.com/api/docs/guides/speech-to-text)、[语音生成](https://developers.openai.com/api/docs/guides/text-to-speech)、[Speech 参数](https://developers.openai.com/api/reference/resources/audio/subresources/speech/methods/create)、[语音架构](https://developers.openai.com/api/docs/guides/voice-agents)。TTS 已暂定，其余具体模型尚未选定，未调用付费 API 实测。

## 双平台验证与发布

macOS 开发，同时从第一天建立 macOS 和 Windows 构建；Windows 用目标平台 CI 或开发机生成安装包，并用实际 Windows 设备验证。Tauri MSI 需要 Windows 工具链，跨平台生成 NSIS 有额外限制，不能靠 Mac 一端验证完成交付。[Tauri Windows 安装包](https://v2.tauri.app/distribute/windows-installer/)

选型前最小样机验证：

1. 焦点在其他应用时按住录音、松开提交，长按不重复触发，异常松键有终止机制。
2. AI 播报中按键立即停止，旧音频不复活；录音第一音节不丢，设备拔插和蓝牙切换能恢复。
3. 主界面隐藏后仍能定时按需截图和播报；关闭窗口与退出程序含义明确。
4. 多屏缩放、窗口移动／遮挡／最小化、权限拒绝和屏幕源失效时行为正确，不返回旧图假装新图。
5. 睡眠唤醒、锁屏和网络恢复后不积压补发大量主动轮次；Windows 游戏／全屏场景单独验证，不默认承诺支持。
6. 在用户设备上测空闲、截图、录音和播放状态的资源占用与实际延迟，再最终确定组件和系统最低版本。

只有这些关键链路通过，再确定 Tauri 方案并扩展完整产品。若原生媒体适配成本明显超出预期，切换 Electron + TypeScript + 原生热键桥；保持截图、音频、会话和存储接口边界，减少迁移影响。
