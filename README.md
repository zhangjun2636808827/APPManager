# AppManager

AppManager 是一个面向 Windows 11 的本地软件管理与启动工具。

当前项目阶段：MVP 功能实现初版。

## 已完成

- Windows 11 风格浅色 UI
- 浏览器预览模式，直接打开 `index.html` 可查看界面
- Tauri 桌面应用骨架
- APP 启动时在 `AppManager.exe` 同目录下创建 `AppManagerLibrary`
- 软件库目录结构：

```text
AppManager.exe
AppManagerLibrary/
  Apps/
  config/
    app-data.json
```

说明：软件库跟随可执行文件所在目录，方便后续做成可迁移的安装包或便携目录，复制到另一台 Windows 电脑后仍能在同目录维护软件库。

- 分类创建
- 分类删除，支持选择是否删除真实文件夹
- 扫描分类目录下的软件文件夹
- 自动识别 `.exe`
- 收藏 / 取消收藏
- 点击启动软件
- 打开软件目录
- 本地 JSON 配置保存

## 预览 UI

直接打开：

```text
preview.html
```

浏览器预览模式不会进行真实文件扫描、文件夹删除或软件启动。

`index.html` 是 Tauri/Vite 正式入口，不建议直接用文件方式打开。

## 运行桌面应用

首次运行前需要安装依赖：

```bash
npm.cmd install
```

启动 Tauri 开发模式：

```bash
npm.cmd run tauri dev
```

桌面应用模式下会启用真实功能。

## 当前限制

- 还没有实现分类重命名
- 多个 `.exe` 时目前只记录待处理问题，尚未提供选择主程序弹窗
- 软件图标暂时使用字母图标，尚未从 `.exe` 提取真实图标
- 深色主题、托盘运行、全局快捷键尚未实现

## 验证状态

已通过：

```bash
node --check src\main.js
cargo fmt --manifest-path src-tauri\Cargo.toml
cargo check --manifest-path src-tauri\Cargo.toml
npm.cmd run build
```
