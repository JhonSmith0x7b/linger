# Linger · YouTube Music 浏览器扩展

Chrome / Edge 120+，复用你已登录的 YouTube Music 标签页。无需提供 Google 密码或 Cookie。仅申请 YouTube Music、回环地址、脚本注入、本地配置和定时唤醒权限，不读取其他网站。

## 安装与配对

1. 打开 Chrome 的 `chrome://extensions`（Edge 为 `edge://extensions`），启用开发者模式。
2. 选择「加载已解压的扩展程序」，选取当前 `youtube-music` 文件夹。
3. 在浏览器打开 `https://music.youtube.com`，登录并手动播放一次，确保网页具备播放权限。
4. 在 Linger 的音乐连接设置中复制配对码。
5. 打开扩展弹窗，粘贴配对码，保留默认端口 `47831`，选择音乐标签页，点击「配对并连接」。
6. 状态显示「已连接 Linger」后，可以对 Linger 说「搜索王菲的歌」「播放第一首」「暂停」「继续」「下一首」「这首后面放第二首」。

音乐标签页关闭后需要重新选择；不会擅自打开、激活或切换标签页。Linger 升级不会自动替换浏览器已加载的扩展文件；更新时需从 Linger 重新导出扩展，替换已加载文件夹，再到扩展管理页面点「重新加载」，核对版本为 0.1.4。配对码是本机控制凭据，不要分享。断开会删除扩展保存的配对设置。

## 行为与边界

- 标签页隐藏时，扩展使用 YouTube Music 的公开曲库搜索，不发送账户 Cookie。目录接口不可用时回退到站内搜索。可见页面的站内搜索会校验关键词、结果刷新和候选稳定性。
- 搜索最多返回 8 首候选（歌曲 ID、标题、详情、时长）。播放和排队使用当前搜索候选 ID。
- 公开曲库候选由绑定页面的站内导航播放；站内导航不可用时打开该曲的播放网址，可能重新加载播放器。网页搜索候选沿用站内歌曲按钮。暂停、继续、下一首和加入队列仍使用网页控件；公开曲库候选入队时会先核对网页搜索结果。
- 播放成功需核对底层歌曲 ID、播放状态和推进的时间；页面标题差异仅作诊断，允许双语标题；排队需在队列中核实该歌曲计数增加。无法验证时返回错误，不能据此宣称操作成功。
- 停止对应暂停，不清空队列。不操作点赞、收藏、歌单或账号设置。
- 命令按顺序执行，收到过的命令不会重复执行；超时表示结果未确认，应查询播放状态而不是重复「下一首」或「加入队列」。
- MV3 后台进程使用短轮询与 30 秒 alarm 唤醒；浏览器/系统休眠时不保证实时执行。恢复后重新建立连接，不补执行过期操作。
- 网站目录接口及内部组件可能变化。搜索、播放和排队的确认超时会显示当前阶段。
- 失败信息包含扩展版本、操作阶段；结果确认超时还包含页面焦点、可见性、搜索路由是否匹配或播放器状态，可用于区分后台页面未提交搜索和播放器未开始播放。诊断字段不包含搜索词、完整网址或页面内容。
- 本地日志仅保留最近 60 条操作，不记录配对码或登录凭据。扩展弹窗也提供手动测试按钮。

## 本地桥接协议

仅访问 `http://127.0.0.1:47831`（端口可在弹窗更改）。每个请求带 `Authorization: Bearer <token>` 和 `X-Linger-Client: <extension-id>`。

- `GET /poll` → `{ "command": null }` 或 `{ "command": { "id": "...", "action": "search", "args": {"query":"..."} } }`
- `POST /result` → `{ "id": "...", "result": { ... } }`，失败使用 `error` 字符串。
- 动作：`search` / `play` / `pause` / `resume` / `next` / `status` / `queue`。

采用扩展 service worker 发出本机请求，配对凭据不注入网页。页面数据只提取歌曲与播放状态；网页文本不是指令。

## 开发检查

```sh
node --check browser-extension/youtube-music/worker.js
node --check browser-extension/youtube-music/page.js
node --check browser-extension/youtube-music/panel.js
```

相关官方文档：[脚本注入](https://developer.chrome.com/docs/extensions/reference/api/scripting)、[扩展后台生命周期](https://developer.chrome.com/docs/extensions/develop/concepts/service-workers/lifecycle)、[定时唤醒](https://developer.chrome.com/docs/extensions/reference/api/alarms)。0.1.4 已在 macOS Chrome 的隐藏音乐标签页验证搜歌、切歌和从暂停状态开始播放。Windows 全屏游戏和后台加入队列仍待实机验证。
