# Windows 测试包

目标为 Windows x64（`x86_64-pc-windows-msvc`）。推荐在 Windows 10/11 上试用；安装器为 NSIS `*-setup.exe`，不是 macOS 应用包改名。

## 在 Windows 本机构建

准备 Node.js、Rust 的 MSVC 工具链、Visual Studio Build Tools 的“使用 C++ 的桌面开发”工作负载（含 Windows SDK）。在项目根目录执行：

```powershell
powershell -File scripts/build-windows.ps1
```

脚本安装锁定的 npm 依赖，添加 x64 Rust target，然后调用 Tauri 构建 release 安装包。调试包可传 `-DebugBuild`。首次构建会联网下载 Rust/Windows 打包依赖。

输出：

- 安装器：`src-tauri/target/x86_64-pc-windows-msvc/release/bundle/nsis/`
- 主程序：`src-tauri/target/x86_64-pc-windows-msvc/release/linger.exe`

安装器会在缺少 WebView2 Runtime 时尝试下载安装。单独复制主程序时，测试机器需已有 WebView2 Runtime。配置、API Key 不包含在安装包中；Windows 上重新填写，Key 保存到 Windows 凭据管理器。

## 从 macOS 交叉编译

使用 LLVM（包括 `clang-cl`、`lld-link`、`llvm-rc`）、NSIS、`cargo-xwin` 和 Windows MSVC Rust target。项目还包含 SQLite、TLS 等原生依赖，不能只安装 Rust target 而省略 C 编译器。

```sh
brew install llvm lld nsis
cargo install --locked cargo-xwin
rustup target add x86_64-pc-windows-msvc
PATH="/opt/homebrew/opt/llvm/bin:/opt/homebrew/opt/lld/bin:$PATH" npm run tauri -- build --runner cargo-xwin --target x86_64-pc-windows-msvc --bundles nsis
```

这里明确指定 NSIS，避免尝试只能在 Windows 上生成的 MSI。交叉编译产物还需 Windows 实机验证；编译成功不等于录音、屏幕捕获、透明窗和全局快捷键已在 Windows 上验证。

参考：[Tauri Windows 安装包与交叉编译](https://v2.tauri.app/distribute/windows-installer/)、[cargo-xwin](https://github.com/rust-cross/cargo-xwin)。

## CI 产物

`.github/workflows/desktop.yml` 的 Windows job 在检查通过后构建 NSIS 包并上传 `Linger-windows-x64` artifact。推送、拉取请求或手动运行会触发检查；构建产物可在对应 GitHub Actions 运行中下载。

## 直接下载安装包

在 [GitHub Releases](https://github.com/JhonSmith0x7b/linger/releases/latest) 下载 `Linger_*_x64-setup.exe`，无需远程访问开发电脑。`SHA256SUMS.txt` 可校验下载文件。

- 推送到 `main`、PR 或手动运行：执行检查并上传 Windows artifact。
- 推送 `v*` 版本标签（例如 `v0.1.2`）：验证标签与应用版本一致，macOS/Windows 检查均通过后，自动创建 Release 并上传 Windows 安装包和校验文件。
- 发布无需个人 Token 或签名证书，使用工作流内置的仓库 Token；只有发布 job 获得写入权限。

发布时先同步 package.json、package-lock.json、Cargo.toml、Cargo.lock 和 tauri.conf.json 中的应用版本，再提交、打标签并推送。当前包未做 Windows 代码签名。
