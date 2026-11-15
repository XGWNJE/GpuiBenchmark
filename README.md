<div align="center">

# GPUI Lab

Android 上的 GPUI 压力测试与样式预览。用多路模拟数据检查长列表滚动、持续更新和组件外观。

[快速开始](#快速开始) · [使用](#使用) · [构建](#构建) · [验证](#验证) · [路线规划](#路线规划)

[![版本 0.1.0](https://img.shields.io/badge/version-0.1.0-blue)](Cargo.toml) [![Android 8+](https://img.shields.io/badge/Android-8%2B-green)](android/app/build.gradle.kts)

</div>

## 快速开始

将 `dist/gpui-lab-arm64-v8a-release.apk` 安装到 ARM64 Android 8.0 及以上设备，打开 **GPUI Lab**。测试包使用 Android 开发签名。

```powershell
adb install -r dist/gpui-lab-arm64-v8a-release.apk
adb shell am start -n dev.gpui.lab/.MainActivity
```

## 使用

- **压力** 提供开始、暂停、自动滚动与参数入口。长列表只绘制可见行。
- **样式** 预览真实 GPUI Kit 按钮、输入、开关、复选框、进度和卡片，可切换浅色、深色、字号与行密度。
- **帮助** 说明测量范围，显示详细结果并通过 Android 分享面板导出 JSON。数据在本机生成，按固定种子更新。

| 负载 | 数据行 | 模拟流 | 总更新量 / 秒 |
| --- | --- | --- | --- |
| 轻量 | 10,000 | 1 | 20 |
| 标准 | 50,000 | 4 | 400 |
| 高压 | 100,000 | 8 | 4,000 |
| 极限 | 200,000 | 16 | 16,000 |

帧回调速率与 P95/P99 使用最近 600 个 CPU 侧帧回调间隔。60/90/120 Hz 设置统计预算，实际刷新率由系统控制。单次最多处理 8,192 条更新，超出量计入跳过次数。切换负载或重置会清空本轮结果，后台暂停更新与采样。GPU 呈现、耗电与真机性能需要结合设备测量。

## 构建

需要 PowerShell、Rust、JDK 17+、Android SDK 36、NDK `28.2.13676358`，并设置 `ANDROID_HOME`。Gradle Wrapper 自动使用 9.3.1。

```powershell
rustup target add aarch64-linux-android x86_64-linux-android
pwsh -File scripts/build-android.ps1
# 模拟器调试包
pwsh -File scripts/build-android.ps1 -Abi x86_64 -Configuration Debug
```

产物与 SHA256 位于 `dist/`。构建目录复用 `.build/`，编译并发为 2，构建缓存预算为 12 GiB。需要回收时运行 `pwsh -File scripts/build-android.ps1 -Clean`。

GPUI 固定为 `0.3.6`，Kit 与移动后端固定 Git 提交，依赖版本见 [Cargo.toml](Cargo.toml) 和 [Cargo.lock](Cargo.lock)。移动集成采用该兼容组合。项目安装了 `gpui-kit` 与 `gpui-kit-design-guides` 两个开发技能，来源由 [skills-lock.json](skills-lock.json) 记录。

本机 API 36 模拟器的 Vulkan 驱动在 Debug 调试标签调用中崩溃。该环境使用 Release 包验证，驱动问题见 [gfxstream #199](https://github.com/google/gfxstream/issues/199)。

## 验证

```powershell
cargo fmt --check
cargo test --lib
```

比较手机表现时使用 ARM64 Release 包，保持相同屏幕刷新率、温度、主题与滚动方式，每档至少运行 60 秒，再暂停并分享结果。样式页可在负载持续运行时切换，检查触控、输入法和滚动。模拟器用于启动及交互回归。

## 路线规划

> **暂无。** 当前没有已公布的未来规划。

## 第三方代码

界面基于 [GPUI Kit](https://github.com/longbridge/gpui-kit) 与 [GPUI](https://github.com/zed-industries/zed)，Android 平台来自 [gpui-mobile](https://github.com/itsbalamurali/gpui-mobile)。复用的 Android 输入宿主保留来源标注，许可见 [android/LICENSE-APACHE](android/LICENSE-APACHE)。
