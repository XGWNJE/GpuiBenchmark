# GPUI Lab

Android GPUI 压力测试与样式预览应用。用户操作与测量说明见 [README](README.md)。

- 在 `main` 推进并本地提交。远端操作遵循用户授权。
- Android 使用 `gpui-mobile` 固定提交，GPUI / Kit 必须保持同一版本。升级时同时核对移动后端与实际组件 API。
- 构建入口为 `pwsh -File scripts/build-android.ps1`。复用 `.build/cargo`，并发 2，构建缓存预算 12 GiB。清理仅处理已核实归属的构建目录，保留产物与必要证据。
- 测试模拟器最多 1 个、数据目录预算 4 GiB。只关闭本项目创建的实例，结束后移除测试数据，复用已安装系统镜像。
- 压力数据与采样必须有界。帧回调间隔、Android 呈现时间和真机表现分别报告。性能结论采用 Release 真机证据。
- 修改模型执行 `cargo test --lib`，修改界面完成 Android 构建及可用环境中的交互检查。按需使用项目安装的 GPUI Kit 技能。
