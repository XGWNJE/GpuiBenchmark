"""GPUI benchmark score service. Uses only the Python standard library."""

from __future__ import annotations

import argparse
from collections import defaultdict, deque
from contextlib import contextmanager
from datetime import datetime, timezone
import hashlib
from http import HTTPStatus
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import ipaddress
import math
from pathlib import Path
import re
import sqlite3
import threading
import time
from urllib.parse import parse_qs, quote, unquote, urlsplit


PROTOCOL_ID = "gpui-benchmark-v1"
MAX_BODY_BYTES = 256 * 1024
RUN_ID_PATTERN = re.compile(r"[A-Za-z0-9_-]{1,128}\Z")
PRESETS = {
    "light": (10_000, 1, 20),
    "medium": (50_000, 4, 400),
    "heavy": (100_000, 8, 4_000),
    "extreme": (200_000, 16, 16_000),
}
SCENES = {"hot", "scroll", "styles_light", "styles_dark"}
EXPECTED_CASES = {
    f"{preset}_{scene}_r{repetition}"
    for preset in PRESETS
    for scene in ("hot", "scroll")
    for repetition in range(1, 4)
} | {
    f"{scene}_r{repetition}"
    for scene in ("styles_light", "styles_dark")
    for repetition in range(1, 4)
}


class InvalidReport(ValueError):
    pass


class RunConflict(ValueError):
    pass


def finite_number(value, field, minimum=0, maximum=1e15):
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        raise InvalidReport(f"{field} 必须是数字")
    if not minimum <= value <= maximum or not math.isfinite(value):
        raise InvalidReport(f"{field} 超出有效范围")
    return float(value)


def integer(value, field, minimum=0, maximum=10**12):
    if isinstance(value, bool) or not isinstance(value, int):
        raise InvalidReport(f"{field} 必须是整数")
    if not minimum <= value <= maximum:
        raise InvalidReport(f"{field} 超出有效范围")
    return value


def text_field(value, field, maximum=128):
    if not isinstance(value, str) or not value.strip() or len(value) > maximum:
        raise InvalidReport(f"{field} 必须是长度 1–{maximum} 的文本")
    if any(ord(character) < 32 for character in value):
        raise InvalidReport(f"{field} 包含控制字符")
    return value.strip()


def object_field(value, field):
    if not isinstance(value, dict):
        raise InvalidReport(f"{field} 必须是对象")
    return value


def boolean(value, field):
    if not isinstance(value, bool):
        raise InvalidReport(f"{field} 必须是布尔值")
    return value


def add_reason(reasons, reason):
    if reason not in reasons:
        reasons.append(reason)


def validate_case(case, refresh_hz, reasons):
    case = object_field(case, "results[]")
    case_id = text_field(case.get("case_id"), "case_id", 64)
    preset = case.get("preset")
    scene = case.get("scene")
    if not isinstance(preset, str) or not isinstance(scene, str) or preset not in PRESETS or scene not in SCENES:
        raise InvalidReport("未知 preset 或 scene")
    repetition = integer(case.get("repetition"), "repetition", 1, 100)
    expected_id = (
        f"{scene}_r{repetition}"
        if scene.startswith("styles_")
        else f"{preset}_{scene}_r{repetition}"
    )
    expected_preset = "extreme" if scene.startswith("styles_") else preset
    expected_rows, expected_streams, expected_rate = PRESETS[expected_preset]
    if case_id != expected_id or preset != expected_preset:
        add_reason(reasons, "用例标识与负载不一致")
    if integer(case.get("rows"), "rows", 1, 10**7) != expected_rows:
        add_reason(reasons, "负载行数不符合标准协议")
    if integer(case.get("streams"), "streams", 1, 1024) != expected_streams:
        add_reason(reasons, "数据流数量不符合标准协议")
    if integer(case.get("configured_updates_per_second"), "configured_updates_per_second", 1, 10**8) != expected_rate:
        add_reason(reasons, "更新速率不符合标准协议")
    warmup_ms = integer(case.get("warmup_ms"), "warmup_ms", 0, 3_600_000)
    measurement_ms = integer(case.get("measurement_ms"), "measurement_ms", 1, 3_600_000)
    actual_ms = finite_number(case.get("actual_duration_ms"), "actual_duration_ms", 0, 3_600_000)
    if warmup_ms != 3000 or measurement_ms != 20_000 or actual_ms < 19_000:
        add_reason(reasons, "采样时长不符合标准协议")

    generated = integer(case.get("generated_updates"), "generated_updates")
    applied = integer(case.get("applied_updates"), "applied_updates")
    dropped = integer(case.get("dropped_updates"), "dropped_updates")
    pending = integer(case.get("pending_updates"), "pending_updates")
    if applied + dropped + pending != generated:
        raise InvalidReport("更新计数相互矛盾")

    frames = object_field(case.get("frame_stats"), "frame_stats")
    if frames.get("measurement") != "frame_callback_intervals":
        add_reason(reasons, "帧指标来源不符合协议")
    frame_count = integer(frames.get("sample_count"), "frame_stats.sample_count", 0, 100_000)
    elapsed = finite_number(frames.get("elapsed_seconds"), "frame_stats.elapsed_seconds", 0, 3600)
    frame_refresh = finite_number(frames.get("refresh_hz"), "frame_stats.refresh_hz", 1, 1000)
    budget = finite_number(frames.get("budget_ms"), "frame_stats.budget_ms", 0.001, 1000)
    fps = finite_number(frames.get("fps"), "frame_stats.fps", 0, 2000)
    average = finite_number(frames.get("average_ms"), "frame_stats.average_ms", 0, 3_600_000)
    p95 = finite_number(frames.get("p95_ms"), "frame_stats.p95_ms", 0, 3_600_000)
    p99 = finite_number(frames.get("p99_ms"), "frame_stats.p99_ms", 0, 3_600_000)
    maximum = finite_number(frames.get("max_ms"), "frame_stats.max_ms", 0, 3_600_000)
    over_count = integer(frames.get("over_budget_count"), "frame_stats.over_budget_count", 0, 100_000)
    ratio = finite_number(frames.get("over_budget_ratio"), "frame_stats.over_budget_ratio", 0, 1)
    overflow = boolean(frames.get("sample_overflow"), "frame_stats.sample_overflow")
    if over_count > frame_count or p95 > p99 or p99 > maximum:
        raise InvalidReport("帧统计相互矛盾")
    if frame_count and abs(ratio - over_count / frame_count) > 0.001:
        raise InvalidReport("超预算比例与计数不一致")
    if frame_count and elapsed > 0 and abs(fps - frame_count / elapsed) > max(0.1, fps * 0.01):
        raise InvalidReport("帧率与采样次数、时间不一致")
    if fps > 0 and abs(average - 1000 / fps) > max(0.01, average * 0.01):
        raise InvalidReport("平均帧间隔与帧率不一致")
    if abs(frame_refresh - refresh_hz) > 0.1 or abs(budget - 1000 / refresh_hz) > 0.1:
        add_reason(reasons, "刷新率或帧预算不一致")
    if overflow:
        add_reason(reasons, "帧采样发生溢出")
    if not frame_count or elapsed <= 0 or average <= 0 or p95 <= 0 or generated == 0:
        add_reason(reasons, "采样数据不足")

    latency = object_field(case.get("queue_latency_ms"), "queue_latency_ms")
    integer(latency.get("sample_count"), "queue_latency_ms.sample_count", 0, 10**8)
    for key in ("average_ms", "p95_ms", "p99_ms", "max_ms"):
        finite_number(latency.get(key), f"queue_latency_ms.{key}", 0, 3_600_000)
    boolean(latency.get("sample_overflow"), "queue_latency_ms.sample_overflow")
    if latency["p95_ms"] > latency["p99_ms"] or latency["p99_ms"] > latency["max_ms"]:
        raise InvalidReport("队列延迟统计相互矛盾")
    if latency["sample_overflow"]:
        add_reason(reasons, "队列延迟采样发生溢出")

    score = 0.0
    if p95 > 0 and generated > 0:
        score = fps * min(1.0, budget / p95) * min(1.0, applied / generated)
    return case_id, score


def validate_report(payload):
    payload = object_field(payload, "report")
    run_id = text_field(payload.get("run_id"), "run_id")
    if not RUN_ID_PATTERN.fullmatch(run_id):
        raise InvalidReport("run_id 只能包含字母、数字、下划线与短横线")
    integer(payload.get("schema_version"), "schema_version", 1, 100)
    text_field(payload.get("protocol_id"), "protocol_id", 64)
    mode = payload.get("mode")
    status = payload.get("status")
    if not isinstance(mode, str) or mode not in {"standard", "demo", "custom"}:
        raise InvalidReport("未知运行模式")
    if not isinstance(status, str) or status not in {"running", "completed", "cancelled", "invalid"}:
        raise InvalidReport("未知运行状态")
    options = object_field(payload.get("options"), "options")
    warmup = integer(options.get("warmup_ms"), "options.warmup_ms", 0, 3_600_000)
    measurement = integer(options.get("measurement_ms"), "options.measurement_ms", 1, 3_600_000)
    repetitions = integer(options.get("repetitions"), "options.repetitions", 1, 100)
    refresh_hz = finite_number(payload.get("refresh_hz"), "refresh_hz", 1, 1000)
    finite_number(payload.get("score"), "score", 0, 1e9)
    invalid_reason = payload.get("invalid_reason")
    if invalid_reason is not None:
        text_field(invalid_reason, "invalid_reason", 512)

    device = object_field(payload.get("device"), "device")
    manufacturer = text_field(device.get("manufacturer"), "device.manufacturer", 96)
    model = text_field(device.get("model"), "device.model", 128)
    text_field(device.get("android"), "device.android", 64)
    integer(device.get("sdk"), "device.sdk", 1, 100)
    text_field(device.get("abi"), "device.abi", 64)
    device_refresh = finite_number(device.get("refresh_hz"), "device.refresh_hz", 1, 1000)
    integer(device.get("width_px"), "device.width_px", 1, 16_384)
    integer(device.get("height_px"), "device.height_px", 1, 16_384)
    is_emulator = boolean(device.get("is_emulator"), "device.is_emulator")

    reasons = []
    if payload["schema_version"] != 2 or payload["protocol_id"] != PROTOCOL_ID:
        add_reason(reasons, "协议版本不匹配")
    if mode != "standard":
        add_reason(reasons, "仅标准测试参与排名")
    if status != "completed":
        add_reason(reasons, "测试尚未完整完成")
    if is_emulator:
        add_reason(reasons, "模拟器成绩仅保留记录")
    if invalid_reason:
        add_reason(reasons, f"客户端标记无效：{invalid_reason}")
    if (warmup, measurement, repetitions) != (3000, 20_000, 3):
        add_reason(reasons, "测试参数不符合标准协议")
    if abs(device_refresh - refresh_hz) > 0.1:
        add_reason(reasons, "设备刷新率与测试刷新率不一致")

    results = payload.get("results")
    if not isinstance(results, list) or len(results) > 300:
        raise InvalidReport("results 必须是至多 300 个用例的数组")
    case_ids = []
    case_scores = []
    for case in results:
        case_id, score = validate_case(case, refresh_hz, reasons)
        case_ids.append(case_id)
        case_scores.append(score)
    if len(case_ids) != len(set(case_ids)):
        raise InvalidReport("用例标识重复")
    if set(case_ids) != EXPECTED_CASES:
        add_reason(reasons, "标准协议要求完整的 30 个用例")
    raw_score = sum(case_scores) / len(case_scores) / 60 * 1000 if case_scores else 0.0
    score = math.floor(raw_score * 100 + 0.5) / 100
    model_key = f"{manufacturer.casefold()}\n{model.casefold()}"
    return run_id, manufacturer, model, model_key, score, reasons


class RunStore:
    def __init__(self, database, public_base_url):
        self.database = str(database)
        self.public_base_url = public_base_url.rstrip("/")
        Path(self.database).parent.mkdir(parents=True, exist_ok=True)
        with self.connect() as connection:
            connection.execute("PRAGMA journal_mode=WAL")
            connection.execute("""
                CREATE TABLE IF NOT EXISTS runs (
                    run_id TEXT PRIMARY KEY,
                    payload_hash TEXT NOT NULL,
                    payload_json TEXT NOT NULL,
                    manufacturer TEXT NOT NULL,
                    model TEXT NOT NULL,
                    model_key TEXT NOT NULL,
                    score REAL NOT NULL,
                    eligible INTEGER NOT NULL,
                    reasons_json TEXT NOT NULL,
                    received_at TEXT NOT NULL,
                    ack_json TEXT NOT NULL DEFAULT '{}'
                )
            """)
            connection.execute("CREATE INDEX IF NOT EXISTS runs_rank ON runs(eligible, model_key, score DESC)")

    @contextmanager
    def connect(self):
        connection = sqlite3.connect(self.database, timeout=15)
        connection.row_factory = sqlite3.Row
        try:
            with connection:
                yield connection
        finally:
            connection.close()

    def _leaderboard(self, connection):
        rows = connection.execute("""
            WITH model_runs AS (
                SELECT *,
                    ROW_NUMBER() OVER (
                        PARTITION BY model_key ORDER BY score DESC, received_at ASC, run_id ASC
                    ) AS position,
                    COUNT(*) OVER (PARTITION BY model_key) AS run_count
                FROM runs WHERE eligible = 1
            )
            SELECT run_id, manufacturer, model, model_key, score, payload_json,
                received_at, run_count
            FROM model_runs WHERE position = 1
            ORDER BY score DESC, received_at ASC, model_key ASC
        """).fetchall()
        entries = []
        previous_score = None
        rank = 0
        for index, row in enumerate(rows, 1):
            if row["score"] != previous_score:
                rank = index
                previous_score = row["score"]
            payload = json.loads(row["payload_json"])
            entries.append({
                "rank": rank,
                "run_id": row["run_id"],
                "manufacturer": row["manufacturer"],
                "model": row["model"],
                "model_key": row["model_key"],
                "score": row["score"],
                "refresh_hz": payload["refresh_hz"],
                "received_at": row["received_at"],
                "run_count": row["run_count"],
            })
        return entries

    def leaderboard(self, model="", limit=100):
        with self.connect() as connection:
            entries = self._leaderboard(connection)
            total_runs = connection.execute("SELECT COUNT(*) FROM runs WHERE eligible = 1").fetchone()[0]
        query = model.strip().casefold()
        filtered = [entry for entry in entries if query in f"{entry['manufacturer']} {entry['model']}".casefold()]
        return {
            "protocol_id": PROTOCOL_ID,
            "measurement": "frame_callback_intervals",
            "provenance": "client_reported",
            "model_count": len(entries),
            "run_count": total_runs,
            "entries": filtered[:limit],
        }

    def submit(self, payload):
        run_id, manufacturer, model, model_key, score, reasons = validate_report(payload)
        canonical = json.dumps(payload, ensure_ascii=False, sort_keys=True, separators=(",", ":"), allow_nan=False)
        digest = hashlib.sha256(canonical.encode("utf-8")).hexdigest()
        received_at = datetime.now(timezone.utc).isoformat(timespec="milliseconds").replace("+00:00", "Z")
        with self.connect() as connection:
            connection.execute("BEGIN IMMEDIATE")
            existing = connection.execute("SELECT payload_hash, ack_json FROM runs WHERE run_id = ?", (run_id,)).fetchone()
            if existing:
                if existing["payload_hash"] != digest:
                    raise RunConflict("同一 run_id 已提交不同内容")
                return json.loads(existing["ack_json"])
            connection.execute("""
                INSERT INTO runs (run_id, payload_hash, payload_json, manufacturer, model,
                    model_key, score, eligible, reasons_json, received_at)
                VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            """, (run_id, digest, canonical, manufacturer, model, model_key, score, int(not reasons),
                  json.dumps(reasons, ensure_ascii=False), received_at))
            best = None
            if not reasons:
                best = next(entry for entry in self._leaderboard(connection) if entry["model_key"] == model_key)
            ack = {
                "run_id": run_id,
                "score": score,
                "rank": best["rank"] if best else None,
                "ranking_eligible": not reasons,
                "reasons": reasons,
                "best_score": best["score"] if best else None,
                "is_model_best": best["run_id"] == run_id if best else False,
                "leaderboard_url": f"{self.public_base_url}/?run={quote(run_id)}",
            }
            connection.execute("UPDATE runs SET ack_json = ? WHERE run_id = ?", (json.dumps(ack, ensure_ascii=False), run_id))
            return ack

    def run(self, run_id):
        with self.connect() as connection:
            row = connection.execute("SELECT payload_json, ack_json, received_at FROM runs WHERE run_id = ?", (run_id,)).fetchone()
        if row is None:
            return None
        return {
            "report": json.loads(row["payload_json"]),
            "submission": json.loads(row["ack_json"]),
            "received_at": row["received_at"],
            "provenance": "client_reported",
        }


class RateLimiter:
    def __init__(self, limit=12, window_seconds=60, max_clients=4096):
        self.limit = limit
        self.window_seconds = window_seconds
        self.max_clients = max_clients
        self.clients = defaultdict(deque)
        self.lock = threading.Lock()

    def allow(self, address):
        now = time.monotonic()
        with self.lock:
            expired = [key for key, times in self.clients.items() if not times or now - times[-1] >= self.window_seconds]
            for key in expired:
                del self.clients[key]
            if address not in self.clients and len(self.clients) >= self.max_clients:
                return False
            times = self.clients[address]
            while times and now - times[0] >= self.window_seconds:
                times.popleft()
            if len(times) >= self.limit:
                return False
            times.append(now)
            return True


class ScoreServer(ThreadingHTTPServer):
    daemon_threads = True

    def __init__(self, address, store, rate_limiter=None):
        self.store = store
        self.rate_limiter = rate_limiter or RateLimiter()
        self.index_html = Path(__file__).with_name("index.html").read_bytes()
        super().__init__(address, ScoreHandler)

    def handle_error(self, request, client_address):
        # Avoid persisting source addresses through the default traceback logger.
        print("GPUI benchmark request handling failed", flush=True)


class ScoreHandler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def setup(self):
        super().setup()
        self.connection.settimeout(15)

    def log_message(self, format, *args):
        # Network addresses remain only in the bounded in-memory rate limiter.
        pass

    def respond(self, status, body, content_type="application/json; charset=utf-8"):
        data = body if isinstance(body, bytes) else json.dumps(body, ensure_ascii=False, allow_nan=False).encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(data)))
        self.send_header("Cache-Control", "no-store")
        self.send_header("X-Content-Type-Options", "nosniff")
        self.send_header("Referrer-Policy", "no-referrer")
        self.send_header("Content-Security-Policy", "default-src 'self'; style-src 'unsafe-inline'; script-src 'unsafe-inline'; img-src 'self' data:; frame-ancestors 'none'; base-uri 'none'")
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        target = urlsplit(self.path)
        if target.path in {"/", "/index.html"}:
            self.respond(200, self.server.index_html, "text/html; charset=utf-8")
        elif target.path == "/healthz":
            self.respond(200, {"status": "ok", "protocol_id": PROTOCOL_ID})
        elif target.path == "/api/leaderboard":
            query = parse_qs(target.query)
            model = query.get("model", [""])[0]
            if len(model) > 128:
                self.respond(400, {"error": "机型筛选过长"})
                return
            self.respond(200, self.server.store.leaderboard(model))
        elif target.path.startswith("/api/runs/"):
            run_id = unquote(target.path[len("/api/runs/"):])
            result = self.server.store.run(run_id) if RUN_ID_PATTERN.fullmatch(run_id) else None
            self.respond(200, result) if result else self.respond(404, {"error": "未找到测试记录"})
        else:
            self.respond(404, {"error": "未找到页面"})

    def do_POST(self):
        if urlsplit(self.path).path != "/api/runs":
            self.close_connection = True
            self.respond(404, {"error": "未找到接口"})
            return
        if self.headers.get("Transfer-Encoding"):
            self.close_connection = True
            self.respond(400, {"error": "请使用 Content-Length 提交 JSON"})
            return
        try:
            length = int(self.headers.get("Content-Length", "-1"))
        except ValueError:
            length = -1
        if length < 0 or length > MAX_BODY_BYTES:
            self.close_connection = True
            self.respond(413 if length > MAX_BODY_BYTES else 411, {"error": "请求体需指定长度且最多 256 KiB"})
            return
        address = self.client_address[0]
        if ipaddress.ip_address(address).is_loopback:
            forwarded = self.headers.get("X-Real-IP")
            if forwarded:
                try:
                    address = str(ipaddress.ip_address(forwarded))
                except ValueError:
                    pass
        if not self.server.rate_limiter.allow(address):
            self.close_connection = True
            self.respond(429, {"error": "提交过于频繁，请稍后重试"})
            return
        if self.headers.get_content_type() != "application/json":
            self.close_connection = True
            self.respond(415, {"error": "请使用 application/json"})
            return
        try:
            raw = self.rfile.read(length)
            if len(raw) != length:
                raise InvalidReport("请求体不完整")
            payload = json.loads(raw.decode("utf-8"), parse_constant=reject_constant, object_pairs_hook=unique_keys)
            result = self.server.store.submit(payload)
        except (InvalidReport, UnicodeDecodeError, json.JSONDecodeError) as error:
            self.respond(400, {"error": str(error)})
        except RecursionError:
            self.respond(400, {"error": "JSON 层级过深"})
        except RunConflict as error:
            self.respond(409, {"error": str(error)})
        except sqlite3.Error:
            self.respond(503, {"error": "存储暂时不可用，请使用相同 run_id 重试"})
        else:
            self.respond(201, result)


def reject_constant(value):
    raise InvalidReport(f"JSON 包含非有限数字 {value}")


def unique_keys(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise InvalidReport(f"JSON 字段重复：{key}")
        result[key] = value
    return result


def main():
    parser = argparse.ArgumentParser(description="GPUI benchmark 测试排行榜")
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, default=8098)
    parser.add_argument("--db", required=True, help="SQLite 数据库绝对路径，置于私有数据目录")
    parser.add_argument("--public-base-url", required=True, help="用户浏览排行榜使用的完整 HTTP(S) 地址")
    args = parser.parse_args()
    public = urlsplit(args.public_base_url)
    if public.scheme not in {"http", "https"} or not public.netloc or public.query or public.fragment:
        parser.error("--public-base-url 需要 HTTP(S) 地址，且不带查询或片段")
    if not Path(args.db).is_absolute():
        parser.error("--db 需要绝对路径")
    store = RunStore(args.db, args.public_base_url)
    server = ScoreServer((args.host, args.port), store)
    print(f"GPUI benchmark listening on {args.host}:{args.port}", flush=True)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()


if __name__ == "__main__":
    main()
