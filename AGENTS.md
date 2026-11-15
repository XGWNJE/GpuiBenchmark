# GPUI benchmark

Android GPUI 自动跑分、压力测试、样式预览与机型排行榜。入口见 [README](README.md)，测量和计分口径见 [测试协议](docs/benchmark.md)。

- 在 `main` 推进并本地提交。远端操作遵循用户授权。
- Android 使用 `gpui-mobile` 固定提交，移动后端与 Kit 必须依赖同一 GPUI 版本。升级时同时核对移动后端与实际组件 API。
- 构建入口为 `pwsh -File scripts/build-android.ps1`。复用 `.build/cargo`，并发 2，构建缓存预算 12 GiB。清理仅处理已核实归属的构建目录，保留产物与必要证据。
- 测试模拟器最多 1 个、数据目录预算 4 GiB。只关闭本项目创建的实例，结束后移除测试数据，复用已安装系统镜像。
- 压力数据与采样必须有界。帧回调间隔、Android 呈现时间和真机表现分别报告。性能结论采用 Release 真机证据。
- 修改模型执行 `cargo test --lib`，修改界面完成 Android 构建及可用环境中的交互检查。按需使用项目安装的 GPUI Kit 技能。
- Benchmark 协议变化同时核对 Rust、Android 宿主、自动采集脚本和 Python 服务。执行主机测试、模拟 ADB 与跨语言契约回归，真实设备结果单独记录。
- 排行榜部署于东京服务器 `gpui-benchmark.service`，代码在 `/opt/gpui-benchmark`，业务数据库在 `/var/lib/gpui-benchmark`。服务和 API 入口见 [服务说明](server/README.md)。
