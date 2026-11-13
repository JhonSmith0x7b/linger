# Linger · YouTube Music 浏览器扩展

Chrome / Edge 120+，复用你已登录的 YouTube Music 标签页。无需提供 Google 密码或 Cookie。仅申请 YouTube Music、回环地址、脚本注入、本地配置和定时唤醒权限，不读取其他网站。

## 安装与配对

1. 打开 Chrome 的 `chrome://extensions`（Edge 为 `edge://extensions`），启用开发者模式。
2. 选择「加载已解压的扩展程序」，选取当前 `youtube-music` 文件夹。
3. 在浏览器打开 `https://music.youtube.com`，登录并手动播放一次，确保网页具备播放权限。
4. 在 Linger 的音乐连接设置中复制配对码。
5. 打开扩展弹窗，粘贴配对码，保留默认端口 `47831`，选择音乐标签页，点击「配对并连接」。
6. 状态显示「已连接 Linger」后，可以对 Linger 说「搜索王菲的歌」「播放第一首」「暂停」「继续」「下一首」「这首后面放第二首」。

音乐标签页关闭后需要重新选择；不会擅自打开、激活或切换标签页。扩展更新后到扩展管理页面点「重新加载」。配对码是本机控制凭据，不要分享。断开会删除扩展保存的配对设置。

## 行为与边界

- 搜索最多返回 8 首候选（歌曲 ID、标题、详情、时长）。播放和排队使用当前搜索候选 ID；结果页被手动更改后需重新搜索。
- 搜索使用站内搜索控件，播放使用站内歌曲播放按钮，暂停/继续/下一首使用播放器按钮。不会直接调用底层 `loadVideoById`，避免音频与页面队列不同步。
- 播放成功需核对底层歌曲 ID、页面标题、播放状态和推进的时间；排队需在队列中核实该歌曲计数增加。无法验证时返回错误，不能据此宣称操作成功。
- 停止对应暂停，不清空队列。不操作点赞、收藏、歌单或账号设置。
- 命令按顺序执行，收到过的命令不会重复执行；超时表示结果未确认，应查询播放状态而不是重复「下一首」或「加入队列」。
- MV3 后台进程使用短轮询与 30 秒 alarm 唤醒；浏览器/系统休眠时不保证实时执行。恢复后重新建立连接，不补执行过期操作。
- 网站 DOM/内部组件可能变化；当前适配失败会明确报错，不退回整页跳转或操作系统鼠标点击。
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

相关官方文档：[脚本注入](https://developer.chrome.com/docs/extensions/reference/api/scripting)、[扩展后台生命周期](https://developer.chrome.com/docs/extensions/develop/concepts/service-workers/lifecycle)、[定时唤醒](https://developer.chrome.com/docs/extensions/reference/api/alarms)。真实登录页面的后台播放、队列和 Windows 游戏场景仍需实际验收。
