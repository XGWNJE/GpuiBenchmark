<div align="center">

# GPUI benchmark

Android GPUI 自动跑分、数据流压力测试与组件样式预览。

[开始使用](#开始使用) · [自动采集](#自动采集) · [测试协议](docs/benchmark.md) · [排行榜](https://visionguard.xgwnje.cn/gpui-benchmark/)

[![版本 0.2.0](https://img.shields.io/badge/version-0.2.0-blue)](Cargo.toml) [![Android 8+](https://img.shields.io/badge/Android-8%2B-green)](android/app/build.gradle.kts)

</div>

## 开始使用

安装 `dist/gpui-benchmark-arm64-v8a-release.apk`，打开 **GPUI benchmark**，点击 **开始跑分**。自动完成 30 个场景，约 12 分钟，随后保存报告并上传标准成绩。安装包使用 Android 开发签名。

固定入口提供 **跑分、压力、样式、帮助**。压力页支持手动选档和自动滚动，样式页使用真实 GPUI Kit 组件。跑分页展示实时回调、数据流、帧间隔和测试进度。

排行榜按机型保留最高标准成绩，可查看单次报告。离线报告保存在应用目录，网络恢复或下次启动后自动补传。测试采样期间暂停上传任务。

## 自动采集

手机建立 ADB 调试连接后执行一条命令，自动选择 ABI、安装、运行完整测试，并收集系统侧证据和 HTML 报告。

```powershell
pwsh -File scripts/benchmark-android.ps1
```

输出复用 `.build/benchmark-results/latest`，预算 256 MiB。设备不支持的系统数据会记录缺失状态。采样范围、计分公式和可选参数见 [测试协议](docs/benchmark.md)。

## 构建与验证

需要 Rust、JDK 17+、Android SDK 36、NDK `28.2.13676358` 和 `ANDROID_HOME`。构建入口复用 `.build/cargo`，并发 2，缓存预算 12 GiB。

```powershell
rustup target add aarch64-linux-android x86_64-linux-android
pwsh -File scripts/build-android.ps1
cargo fmt --check
cargo test --lib --target-dir .build/model-tests
python -B -m unittest discover -s tests -v
python -B -m unittest discover -s server -v
```

产物及 SHA256 位于 `dist/`。`-Abi x86_64` 可构建模拟器包，`-Clean` 回收项目构建缓存。本轮执行代码、模拟 ADB 和构建回归，手机自动跑分及上传链路待实机验证。排行榜网页、API 和健康检查已部署并验证。

## 项目结构

- [测试协议与统计口径](docs/benchmark.md)
- [排行榜服务与 API](server/README.md)
- [Agent 协作入口](AGENTS.md)

界面基于 [GPUI Kit](https://github.com/longbridge/gpui-kit) 和 [GPUI](https://github.com/zed-industries/zed)，Android 使用社区 [gpui-mobile](https://github.com/itsbalamurali/gpui-mobile)。依赖固定版本及提交见 [Cargo.toml](Cargo.toml)。项目级技能来源见 [skills-lock.json](skills-lock.json)，Android 输入宿主保留来源与 [许可](android/LICENSE-APACHE)。
