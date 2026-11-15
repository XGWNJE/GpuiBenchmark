import copy
from concurrent.futures import ThreadPoolExecutor
import http.client
import json
from pathlib import Path
import tempfile
import threading
import unittest

from main import EXPECTED_CASES, InvalidReport, MAX_BODY_BYTES, PRESETS, RateLimiter, RunConflict, RunStore, ScoreServer, validate_report


def temporary_directory():
    root = Path(__file__).resolve().parent.parent / ".build" / "server-tests"
    root.mkdir(parents=True, exist_ok=True)
    return tempfile.TemporaryDirectory(dir=root)


def report(run_id="run-test-1", model="Test Phone", fps=60.0):
    results = []
    for case_id in sorted(EXPECTED_CASES):
        scene_key, repeat = case_id.rsplit("_r", 1)
        if scene_key.startswith("styles_"):
            preset, scene = "extreme", scene_key
        else:
            preset, scene = scene_key.rsplit("_", 1)
        rows, streams, rate = PRESETS[preset]
        average = 1000 / fps
        results.append({
            "case_id": case_id, "preset": preset, "scene": scene, "repetition": int(repeat),
            "warmup_ms": 3000, "measurement_ms": 20_000, "actual_duration_ms": 20_001.0,
            "rows": rows, "streams": streams, "configured_updates_per_second": rate,
            "generated_updates": rate * 20, "applied_updates": rate * 20,
            "dropped_updates": 0, "pending_updates": 0,
            "frame_stats": {
                "measurement": "frame_callback_intervals", "sample_count": int(fps * 20),
                "elapsed_seconds": 20.0, "refresh_hz": 60.0, "budget_ms": 1000 / 60,
                "fps": fps, "average_ms": average, "p95_ms": average,
                "p99_ms": average, "max_ms": average,
                "over_budget_count": int(fps * 20) if average > 1000 / 60 else 0,
                "over_budget_ratio": 1.0 if average > 1000 / 60 else 0.0,
                "sample_overflow": False,
            },
            "queue_latency_ms": {
                "sample_count": int(fps * 20), "average_ms": 2.0, "p95_ms": 3.0,
                "p99_ms": 4.0, "max_ms": 5.0, "sample_overflow": False,
            },
        })
    return {
        "schema_version": 2, "protocol_id": "gpui-benchmark-v1", "run_id": run_id,
        "mode": "standard", "status": "completed",
        "options": {"warmup_ms": 3000, "measurement_ms": 20_000, "repetitions": 3},
        "refresh_hz": 60.0, "score": 999999.0, "invalid_reason": None,
        "device": {
            "manufacturer": "Test", "model": model, "android": "16", "sdk": 36,
            "abi": "arm64-v8a", "refresh_hz": 60.0, "width_px": 1080,
            "height_px": 2340, "is_emulator": False,
        },
        "results": results,
    }


class StoreTests(unittest.TestCase):
    def setUp(self):
        self.temp = temporary_directory()
        self.store = RunStore(Path(self.temp.name) / "scores.sqlite3", "https://example.test/gpui-benchmark")

    def tearDown(self):
        self.temp.cleanup()

    def test_server_recomputes_score_and_penalties(self):
        payload = report()
        self.assertEqual(self.store.submit(payload)["score"], 1000.0)
        payload = report("penalty-run")
        for case in payload["results"]:
            case["applied_updates"] //= 2
            case["dropped_updates"] = case["applied_updates"]
            case["frame_stats"]["p95_ms"] *= 2
            case["frame_stats"]["p99_ms"] *= 2
            case["frame_stats"]["max_ms"] *= 2
        self.assertEqual(self.store.submit(payload)["score"], 250.0)

    def test_idempotency_and_conflict(self):
        payload = report()
        first = self.store.submit(payload)
        self.store.submit(report("another-model", model="Faster Phone", fps=120))
        reordered = dict(reversed(list(payload.items())))
        self.assertEqual(first, self.store.submit(reordered))
        payload["score"] = 0
        with self.assertRaises(RunConflict):
            self.store.submit(payload)
        self.assertEqual(self.store.leaderboard()["run_count"], 2)

    def test_best_score_per_model_and_filter(self):
        self.store.submit(report("slow-first", model="Phone One", fps=30))
        self.store.submit(report("fast-second", model="Phone One", fps=60))
        self.store.submit(report("other-model", model="Phone Two", fps=45))
        board = self.store.leaderboard()
        self.assertEqual(board["model_count"], 2)
        self.assertEqual(board["entries"][0]["run_id"], "fast-second")
        self.assertEqual(board["entries"][0]["run_count"], 2)
        self.assertEqual(self.store.leaderboard("two")["entries"][0]["rank"], 2)

    def test_ineligible_runs_are_preserved_with_reasons(self):
        for name, mutate in [
            ("demo", lambda value: value.update(mode="demo")),
            ("emulator", lambda value: value["device"].update(is_emulator=True)),
            ("incomplete", lambda value: value["results"].pop()),
            ("cancelled", lambda value: value.update(status="cancelled")),
        ]:
            payload = report(name)
            mutate(payload)
            ack = self.store.submit(payload)
            self.assertFalse(ack["ranking_eligible"])
            self.assertIsNone(ack["rank"])
            self.assertTrue(ack["reasons"])
            self.assertEqual(self.store.run(name)["report"], payload)
        self.assertEqual(self.store.leaderboard()["entries"], [])

    def test_nonfinite_invalid_and_inconsistent_statistics(self):
        payload = report()
        payload["results"][0]["frame_stats"]["fps"] = float("nan")
        with self.assertRaises(InvalidReport):
            validate_report(payload)

    def test_malformed_enum_and_large_number_are_validation_errors(self):
        payload = report()
        payload["mode"] = []
        with self.assertRaises(InvalidReport):
            validate_report(payload)
        payload = report()
        payload["score"] = 10**1000
        with self.assertRaises(InvalidReport):
            validate_report(payload)
        payload = report()
        payload["results"][0]["frame_stats"]["fps"] *= 2
        with self.assertRaises(InvalidReport):
            validate_report(payload)
        payload = report()
        payload["results"][0]["applied_updates"] += 1
        with self.assertRaises(InvalidReport):
            validate_report(payload)
        payload = report()
        payload["results"].append(copy.deepcopy(payload["results"][0]))
        with self.assertRaises(InvalidReport):
            validate_report(payload)

    def test_concurrent_duplicate_posts_have_one_record(self):
        payload = report()
        with ThreadPoolExecutor(max_workers=8) as pool:
            responses = list(pool.map(lambda _: self.store.submit(payload), range(16)))
        self.assertTrue(all(value == responses[0] for value in responses))
        self.assertEqual(self.store.leaderboard()["run_count"], 1)

    def test_concurrent_distinct_models_remain_consistent(self):
        with ThreadPoolExecutor(max_workers=6) as pool:
            list(pool.map(lambda index: self.store.submit(report(f"run-{index}", f"Phone {index}")), range(12)))
        board = self.store.leaderboard()
        self.assertEqual(board["run_count"], 12)
        self.assertEqual(board["model_count"], 12)
        self.assertEqual({entry["rank"] for entry in board["entries"]}, {1})


class HttpTests(unittest.TestCase):
    def setUp(self):
        self.temp = temporary_directory()
        store = RunStore(Path(self.temp.name) / "scores.sqlite3", "https://example.test/gpui-benchmark")
        self.server = ScoreServer(("127.0.0.1", 0), store)
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()

    def tearDown(self):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=3)
        self.temp.cleanup()

    def request(self, method, path, body=None, headers=None):
        connection = http.client.HTTPConnection("127.0.0.1", self.server.server_port, timeout=3)
        connection.request(method, path, body, headers or {})
        response = connection.getresponse()
        result = response.status, response.read(), dict(response.getheaders())
        connection.close()
        return result

    def post(self, payload, extra_headers=None):
        headers = {"Content-Type": "application/json"}
        headers.update(extra_headers or {})
        return self.request("POST", "/api/runs", json.dumps(payload).encode(), headers)

    def test_roundtrip_health_leaderboard_and_detail(self):
        self.assertEqual(self.request("GET", "/healthz")[0], 200)
        status, body, _ = self.post(report())
        self.assertEqual(status, 201)
        ack = json.loads(body)
        self.assertEqual(ack["leaderboard_url"], "https://example.test/gpui-benchmark/?run=run-test-1")
        self.assertEqual(self.request("GET", "/api/runs/run-test-1")[0], 200)
        self.assertEqual(len(json.loads(self.request("GET", "/api/leaderboard")[1])["entries"]), 1)
        self.assertEqual(self.request("GET", "/api/runs/missing")[0], 404)

    def test_body_limit_and_json_errors(self):
        status, _, _ = self.request("POST", "/api/runs", b"", {"Content-Type": "application/json", "Content-Length": str(MAX_BODY_BYTES + 1)})
        self.assertEqual(status, 413)
        self.assertEqual(self.request("POST", "/api/runs", b'{"x":NaN}', {"Content-Type": "application/json"})[0], 400)
        self.assertEqual(self.request("POST", "/api/runs", b'{"x":1,"x":2}', {"Content-Type": "application/json"})[0], 400)
        self.assertEqual(self.post({})[0], 400)

    def test_conflict_and_rate_limit(self):
        self.server.rate_limiter = RateLimiter(limit=2)
        payload = report()
        self.assertEqual(self.post(payload)[0], 201)
        payload["score"] = 0
        self.assertEqual(self.post(payload)[0], 409)
        self.assertEqual(self.post(payload)[0], 429)

    def test_concurrent_http_posts_are_idempotent(self):
        self.server.rate_limiter = RateLimiter(limit=32)
        payload = report("http-concurrent")
        with ThreadPoolExecutor(max_workers=6) as pool:
            responses = list(pool.map(lambda _: self.post(payload), range(16)))
        self.assertTrue(all(status == 201 for status, _, _ in responses))
        acknowledgements = [json.loads(body) for _, body, _ in responses]
        self.assertTrue(all(value == acknowledgements[0] for value in acknowledgements))
        self.assertEqual(self.server.store.leaderboard()["run_count"], 1)

    def test_forwarded_ip_is_memory_only_and_bounded(self):
        self.server.rate_limiter = RateLimiter(limit=1, max_clients=2)
        self.assertEqual(self.post(report("first"), {"X-Real-IP": "192.0.2.1"})[0], 201)
        self.assertEqual(self.post(report("second"), {"X-Real-IP": "192.0.2.2"})[0], 201)
        self.assertEqual(self.post(report("third"), {"X-Real-IP": "192.0.2.3"})[0], 429)
        self.assertLessEqual(len(self.server.rate_limiter.clients), 2)
        with self.server.store.connect() as connection:
            columns = [row[1] for row in connection.execute("PRAGMA table_info(runs)")]
        self.assertFalse(any("ip" in name or "address" in name for name in columns))

    def test_html_uses_relative_api_paths_and_safe_dom(self):
        status, body, headers = self.request("GET", "/")
        self.assertEqual(status, 200)
        self.assertIn(b"fetch('./api/leaderboard')", body)
        self.assertNotIn(b"innerHTML", body)
        self.assertEqual(headers["X-Content-Type-Options"], "nosniff")


if __name__ == "__main__":
    unittest.main()
