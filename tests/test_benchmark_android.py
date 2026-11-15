import importlib.util
import json
from pathlib import Path
import queue
import subprocess
import tempfile
import unittest
from unittest.mock import MagicMock, patch


SCRIPT = Path(__file__).resolve().parents[1] / "scripts" / "benchmark_android.py"
SPEC = importlib.util.spec_from_file_location("benchmark_android", SCRIPT)
benchmark = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(benchmark)


SURFACE = """SurfaceFlinger TimeStats:
statsStart = 100
totalFrames = 900
layerName = SurfaceView[dev.gpui.lab/dev.gpui.lab.MainActivity](BLAST)#123
packageName = dev.gpui.lab
totalFrames = {frames}
averageFPS = 60.0
presentToPresent histogram is as below:
8ms={fast} 9ms=0 16ms={slow}
layerName = SurfaceView[other.app/MainActivity]#124
totalFrames = 800
presentToPresent histogram is as below:
16ms=799
"""


class DeviceSelectionTests(unittest.TestCase):
    def test_no_device_has_actionable_failure(self):
        with self.assertRaisesRegex(benchmark.BenchmarkError, "没有可用 ADB"):
            benchmark.select_device("List of devices attached\n")

    def test_multiple_devices_require_target(self):
        devices = "List of devices attached\nphone device product:x\nemulator-5560 device product:y\n"
        with self.assertRaisesRegex(benchmark.BenchmarkError, "多个设备"):
            benchmark.select_device(devices)
        self.assertEqual(benchmark.select_device(devices, "phone"), "phone")

    def test_unauthorized_is_not_selected(self):
        with self.assertRaises(benchmark.BenchmarkError):
            benchmark.select_device("phone unauthorized\n", "phone")

    def test_native_emulator_abi_precedes_arm_translation(self):
        with tempfile.TemporaryDirectory() as folder:
            repository = Path(folder)
            (repository / "dist").mkdir()
            x86 = repository / "dist" / "gpui-benchmark-x86_64-release.apk"
            arm = repository / "dist" / "gpui-benchmark-arm64-v8a-release.apk"
            x86.write_bytes(b"apk")
            arm.write_bytes(b"apk")
            self.assertEqual(benchmark.select_apk("x86_64,arm64-v8a", repository), x86)
            self.assertEqual(benchmark.select_apk("arm64-v8a,armeabi-v7a", repository), arm)


class EvidenceTests(unittest.TestCase):
    def test_rejects_report_from_previous_run(self):
        report = {"run_id": "old", "schema_version": 2, "protocol_id": "gpui-benchmark-v1", "status": "cancelled", "results": []}
        with self.assertRaisesRegex(benchmark.BenchmarkError, "run_id"):
            benchmark.validate_report(report, "current")
        report["run_id"] = "current"
        self.assertIs(benchmark.validate_report(report, "current"), report)

    def test_completed_report_requires_all_unique_cases(self):
        report = {
            "run_id": "run", "schema_version": 2, "protocol_id": "gpui-benchmark-v1",
            "status": "completed", "options": {"repetitions": 1},
            "results": [{"case_id": str(index)} for index in range(9)],
        }
        with self.assertRaisesRegex(benchmark.BenchmarkError, "场景数量"):
            benchmark.validate_report(report, "run")
        report["results"].append({"case_id": "9"})
        self.assertIs(benchmark.validate_report(report, "run"), report)
        report["results"][-1]["case_id"] = "0"
        with self.assertRaises(benchmark.BenchmarkError):
            benchmark.validate_report(report, "run")

    def test_surface_parser_filters_app_and_uses_histogram_delta(self):
        before = SURFACE.format(frames=11, fast=8, slow=2)
        after = SURFACE.format(frames=31, fast=26, slow=4)
        layers = benchmark.surface_delta(before, after)
        self.assertEqual(len(layers), 1)
        self.assertEqual(layers[0]["status"], "available")
        self.assertEqual(layers[0]["total_frames_delta"], 20)
        self.assertEqual(layers[0]["present_to_present_samples"], 20)
        self.assertAlmostEqual(layers[0]["average_fps_from_histogram"], 1000 * 20 / 176)

    def test_reset_counters_are_marked_unavailable(self):
        layers = benchmark.surface_delta(
            SURFACE.format(frames=31, fast=26, slow=4),
            SURFACE.format(frames=11, fast=8, slow=2),
        )
        self.assertEqual(layers[0]["status"], "counters_reset")

    def test_output_cleanup_is_limited_to_owned_flat_artifacts(self):
        with tempfile.TemporaryDirectory() as folder:
            output = Path(folder) / "results"
            benchmark.prepare_output(output)
            (output / "report.html").write_text("old", encoding="utf-8")
            benchmark.prepare_output(output)
            self.assertFalse((output / "report.html").exists())
            (output / "user-file.txt").write_text("preserve", encoding="utf-8")
            with self.assertRaises(benchmark.BenchmarkError):
                benchmark.prepare_output(output)
            self.assertEqual((output / "user-file.txt").read_text(), "preserve")

    def test_pull_rejects_budget_overflow_before_copy(self):
        adb = MagicMock()
        adb.run.return_value = str(benchmark.OUTPUT_BUDGET + 1)
        with tempfile.TemporaryDirectory() as folder:
            with self.assertRaisesRegex(benchmark.BenchmarkError, "256 MiB"):
                benchmark.pull_bounded(adb, "/owned-trace", Path(folder) / "trace.pftrace")
        adb.run.assert_called_once()

    def test_html_escapes_device_content_and_discloses_system_scope(self):
        report = {"protocol_id": "gpui-benchmark-v1", "score": 1000, "status": "completed", "results": []}
        rendered = benchmark.render_html(report, {}, {"model": "<script>alert(1)</script>"})
        self.assertNotIn("<script>alert(1)</script>", rendered)
        self.assertIn("包含启动与预热", rendered)
        self.assertIn("128 MiB", rendered)


class CaptureLifecycleTests(unittest.TestCase):
    def test_completion_from_another_run_is_ignored(self):
        events = MagicMock()
        events.events = queue.Queue()
        events.events.put("BENCHMARK_COMPLETE previous")
        events.events.put("BENCHMARK_COMPLETE current")
        events.process.poll.return_value = None
        benchmark.wait_for_completion(MagicMock(), events, "current", duration=1)
        self.assertTrue(events.events.empty())

    def test_unsupported_perfetto_preserves_unavailable_status(self):
        adb = MagicMock()
        adb.run.side_effect = ["Permission Denial", benchmark.BenchmarkError("perfetto unavailable")]
        with tempfile.TemporaryDirectory() as folder:
            capture = benchmark.SystemCapture(adb, Path(folder), "run", 120)
            capture.start()
            capture.close()
        self.assertEqual(capture.coverage["perfetto"]["status"], "unavailable")
        self.assertFalse(capture.coverage["perfetto"]["app_frame_timeline_verified"])
        self.assertFalse(capture.surface_owned)

    def test_existing_surface_statistics_are_preserved(self):
        adb = MagicMock()
        adb.run.side_effect = [SURFACE.format(frames=11, fast=8, slow=2), benchmark.BenchmarkError("unsupported")]
        with tempfile.TemporaryDirectory() as folder:
            capture = benchmark.SystemCapture(adb, Path(folder), "run", 120)
            capture.start()
            capture.close()
        arguments = [call.args for call in adb.run.call_args_list]
        self.assertFalse(any("-enable" in command or "-disable" in command or "-clear" in command for command in arguments))

    def test_owned_surface_enable_is_disabled_on_close(self):
        adb = MagicMock()
        adb.run.side_effect = ["", "", benchmark.BenchmarkError("unsupported"), ""]
        with tempfile.TemporaryDirectory() as folder:
            capture = benchmark.SystemCapture(adb, Path(folder), "run", 120)
            capture.start()
            capture.close()
        self.assertIn(("shell", "dumpsys", "SurfaceFlinger", "--timestats", "-disable"),
                      [call.args for call in adb.run.call_args_list])

    def test_trace_stop_checks_exact_owned_capture_path(self):
        adb = MagicMock()
        with tempfile.TemporaryDirectory() as folder, patch.object(benchmark.time, "sleep"):
            capture = benchmark.SystemCapture(adb, Path(folder), "run", 120)
            capture.trace_pid = "123"
            adb.run.return_value = "/system/bin/perfetto\0-o\0/another-session.pftrace"
            capture.stop_trace()
            self.assertFalse(any("kill" in call.args for call in adb.run.call_args_list))
            adb.run.return_value = f"/system/bin/perfetto\0-o\0{capture.remote_trace}"
            capture.stop_trace()
        self.assertIn(("shell", "kill", "-INT", "123"), [call.args for call in adb.run.call_args_list])

    def test_timeout_closes_owned_collectors_and_stops_app(self):
        adb = MagicMock()
        capture = MagicMock()
        capture.coverage = {"cleanup_errors": []}
        events = MagicMock()
        with tempfile.TemporaryDirectory() as folder, \
                patch.object(benchmark, "collect_device", return_value={"abi_list": "arm64-v8a"}), \
                patch.object(benchmark, "select_apk", return_value=Path("app.apk")), \
                patch.object(benchmark, "collect_snapshot"), \
                patch.object(benchmark, "SystemCapture", return_value=capture), \
                patch.object(benchmark, "EventStream", return_value=events), \
                patch.object(benchmark, "wait_for_completion", side_effect=benchmark.BenchmarkError("timeout")):
            with self.assertRaisesRegex(benchmark.BenchmarkError, "timeout"):
                benchmark.run_benchmark(adb, Path(folder) / "result", quick=True)
            events.close.assert_called_once()
            capture.close.assert_called_once()
            self.assertTrue((Path(folder) / "result" / "failure.txt").exists())
        self.assertIn(("shell", "am", "force-stop", benchmark.PACKAGE),
                      [call.args for call in adb.run.call_args_list])

    def test_adb_timeout_becomes_bounded_error(self):
        with patch.object(subprocess, "run", side_effect=subprocess.TimeoutExpired(["adb"], 5)):
            with self.assertRaisesRegex(benchmark.BenchmarkError, "超时"):
                benchmark.Adb("adb", "phone").run("shell", "pidof", benchmark.PACKAGE, timeout=5)


if __name__ == "__main__":
    unittest.main()
