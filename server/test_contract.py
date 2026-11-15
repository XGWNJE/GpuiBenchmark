"""Cross-check the real Rust JSON contract without building or running Android."""

import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

from main import EXPECTED_CASES, RunStore


ROOT = Path(__file__).resolve().parent.parent


class SerializationContractTests(unittest.TestCase):
    def test_real_rust_serialization_is_accepted_and_scores_match(self):
        rustc = shutil.which("rustc")
        dependencies = ROOT / ".build" / "model-tests" / "debug" / "deps"
        serde = list(dependencies.glob("libserde-*.rlib"))
        serde_json = list(dependencies.glob("libserde_json-*.rlib"))
        if not rustc or not serde or not serde_json:
            self.skipTest("先运行 cargo test --lib --target-dir .build/model-tests 生成主机依赖")
        scratch = ROOT / ".build" / "server-tests"
        scratch.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(dir=scratch) as temporary:
            subprocess.run([
                rustc, "--edition=2021", "-A", "dead_code",
                str(ROOT / "server" / "contract_fixture.rs"),
                "--extern", f"serde={serde[0]}",
                "--extern", f"serde_json={serde_json[0]}",
                "-L", f"dependency={dependencies}",
                "--out-dir", temporary,
            ], check=True, capture_output=True, text=True, timeout=60)
            executable = Path(temporary) / ("contract_fixture.exe" if os.name == "nt" else "contract_fixture")
            result = subprocess.run([str(executable)], check=True, capture_output=True, text=True, timeout=10)
            payload = json.loads(result.stdout)
            payload["device"] = {
                "manufacturer": "Contract test", "model": "Synthetic fixture",
                "android": "16", "sdk": 36, "abi": "arm64-v8a",
                "refresh_hz": 120.0, "width_px": 1080, "height_px": 2340,
                "is_emulator": False,
            }
            payload["app_version"] = "0.2.0"
            payload["generated_at"] = "2026-11-15T00:00:00Z"
            self.assertEqual({case["case_id"] for case in payload["results"]}, EXPECTED_CASES)
            self.assertEqual({case["repetition"] for case in payload["results"]}, {1, 2, 3})
            store = RunStore(Path(temporary) / "contract.sqlite3", "https://example.test/gpui-benchmark/")
            acknowledgement = store.submit(payload)
            self.assertTrue(acknowledgement["ranking_eligible"], acknowledgement["reasons"])
            self.assertEqual(acknowledgement["score"], payload["score"])
            self.assertEqual(acknowledgement["rank"], 1)
            self.assertEqual(store.run(payload["run_id"])["report"], payload)

    def test_android_metadata_keys_match_the_server_contract(self):
        source = (ROOT / "android" / "app" / "src" / "main" / "java" / "dev" / "gpui" / "lab" / "BenchmarkHost.java").read_text(encoding="utf-8")
        for key in ("manufacturer", "model", "android", "sdk", "abi", "refresh_hz", "width_px", "height_px", "is_emulator"):
            self.assertIn(f'result.put("{key}",', source)
        self.assertIn('report.put("device", device)', source)
        self.assertIn('report.put("generated_at",', source)
        self.assertIn('"https://visionguard.xgwnje.cn/gpui-benchmark/"', source)


if __name__ == "__main__":
    unittest.main()
