# 排行榜服务

Python 3.10+ 标准库 HTTP 服务与 SQLite，静态页面和 API 使用同一入口。

已部署入口为 [GPUI benchmark](https://visionguard.xgwnje.cn/gpui-benchmark/)。东京服务器使用 `gpui-benchmark.service`，代码目录 `/opt/gpui-benchmark`，SQLite 位于 `/var/lib/gpui-benchmark/scores.sqlite3`。

```powershell
python server/main.py --host 127.0.0.1 --port 8098 --db C:/private/gpui-benchmark/scores.sqlite3 --public-base-url https://visionguard.xgwnje.cn/gpui-benchmark/
python -m unittest discover -s server -v
```

`test_contract.py` 使用应用中的真实 Rust 序列化类型验证标准报告、30 个用例编号及服务端计分一致性。它复用 `cargo test --lib --target-dir .build/model-tests` 的主机依赖，不构建 Android；主机依赖未生成时跳过该项。合成数据仅写入随用随清的本地临时 SQLite。

反向代理将 `/gpui-benchmark/` 前缀去掉后转发到此服务，并设置 `X-Real-IP` 为连接来源地址。后端只信任回环连接提供的该头。公开地址需按实际部署传入，SQLite 路径使用私有目录中的绝对路径。

- `POST /api/runs` 接收 `schema_version=2`、`protocol_id=gpui-benchmark-v1` 的报告，最多 256 KiB。合法记录返回 HTTP 201，响应包含 `run_id`、`score`、`rank`、`ranking_eligible`、`reasons`、`best_score`、`is_model_best`、`leaderboard_url`。
- 同一 `run_id`、相同 JSON 内容返回首次响应。内容冲突返回 409。`rank` 是提交时该机型最佳成绩的名次，页面使用最新榜单。并列分数采用并列名次。
- `GET /api/leaderboard?model=关键词` 返回机型最佳成绩，最多 100 个。`GET /api/runs/<run_id>` 返回原报告、提交结果和服务端接收时间。`GET /healthz` 返回存活状态。
- 标准测试必须覆盖 30 个用例，每项预热 3 秒、测量 20 秒。演示、自定义、模拟器和不完整记录保存明确原因，排名为空。服务端按原始统计重新计分。
- 数据为客户端自报。设备身份、帧统计和跑分真实性没有远程证明，公开页对此有说明。地址仅保存在有界限流内存中，每个来源每分钟最多提交 12 次，最多保留 4096 个来源。

服务使用 WAL 和单次提交事务保存记录及幂等响应。备份时使用 SQLite 备份接口，或在停止服务后一起处理数据库及其 WAL 文件。
