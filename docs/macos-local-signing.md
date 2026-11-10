# macOS 本地开发签名与权限

用于日常体验的本地调试包建议通过以下命令构建，并始终从同一位置打开生成的 `Linger.app`：

```sh
npm run desktop -- build --debug --bundles app
```

当未指定 `APPLE_SIGNING_IDENTITY`，且本机仅有一个有效的 Apple Development 签名身份时，脚本会自动交给 Tauri 为该调试应用包签名。显式设置的身份优先；不修改钥匙串信任、不创建或导入证书，不自动选择发布证书。无有效开发身份或有多个候选时只提示配置，不擅自决定身份。`tauri dev`、普通发布构建及 Windows 构建不使用这项自动选择。

原先的包只有链接器 ad-hoc 签名，身份要求绑定当前可执行文件的哈希值。重编译会改变该值，macOS 无法稳定把新版与旧版权限授权关联起来。固定 Bundle ID 本身不足以解决此问题。使用稳定开发证书后，首个签名版本可能仍需重新授权屏幕、麦克风或钥匙串；旧 ad-hoc 版本的授权不保证迁移。之后同一签名身份的更新才能被系统识别。不要交替使用原始 `target/debug/linger`、不同签名渠道和多份路径的应用来验证权限持久化。

保留 Hardened Runtime，并只加入 `com.apple.security.device.audio-input` 供已有麦克风功能使用。它不能代替用户的系统麦克风授权。首次使用签名私钥时，macOS 也可能要求允许 `codesign` 访问该私钥；这与运行应用时读取用户 API Key 的提示不同。

验证应用身份：

```sh
codesign --verify --deep --strict src-tauri/target/debug/bundle/macos/Linger.app
codesign --display --requirements - src-tauri/target/debug/bundle/macos/Linger.app
codesign --display --entitlements - src-tauri/target/debug/bundle/macos/Linger.app
```

这不是发布签名或公证流程，也不承诺消除系统按策略要求的重新授权。如果未重新构建、未改变路径或签名的同一个应用仍反复弹窗，应另查钥匙串项目 ACL、系统权限状态和实际执行的应用副本，不能仅归因于重编译。

依据：[Apple TN3127：代码身份与权限](https://developer.apple.com/documentation/technotes/tn3127-inside-code-signing-requirements/)、[Apple DTS：ad-hoc 重编译导致录屏权限失效](https://developer.apple.com/forums/thread/819406)、[Tauri macOS 签名配置](https://v2.tauri.app/distribute/sign/macos/)、[Apple Audio Input entitlement](https://developer.apple.com/documentation/bundleresources/entitlements/com.apple.security.device.audio-input)。
