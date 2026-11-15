#!/usr/bin/env python3
"""Run GPUI benchmark and collect independent Android evidence with one command."""

from __future__ import annotations

import argparse
import html
import json
import os
from pathlib import Path
import queue
import re
import shutil
import subprocess
import sys
import threading
import time
import uuid

PACKAGE = "dev.gpui.lab"
ACTIVITY = f"{PACKAGE}/.MainActivity"
REMOTE_REPORT = f"/sdcard/Android/data/{PACKAGE}/files/benchmark/latest.json"
OUTPUT_BUDGET = 256 * 1024 * 1024
TRACE_BUFFER_KIB = 128 * 1024
LOG_BUDGET = 4 * 1024 * 1024
REPO_ROOT = Path(__file__).resolve().parents[1]
OWNED_FILES = {
    "owner.json", "app-report.json", "system-coverage.json", "summary.json",
    "report.html", "trace.pftrace", "trace-analysis.csv", "events.log",
    "device.json", "display.txt", "meminfo-before.txt", "meminfo-after.txt",
    "thermal-before.txt", "thermal-after.txt", "surfaceflinger-before.txt",
    "surfaceflinger-after.txt", "perfetto-start.txt", "failure.txt",
}


class BenchmarkError(RuntimeError):
    pass


class Adb:
    def __init__(self, executable: str, serial: str | None = None):
        self.executable = executable
        self.serial = serial

    def command(self, *arguments: str) -> list[str]:
        target = ["-s", self.serial] if self.serial else []
        return [self.executable, *target, *arguments]

    def run(self, *arguments: str, timeout: float = 20,
            input_text: str | None = None, check: bool = True) -> str:
        try:
            result = subprocess.run(
                self.command(*arguments), input=input_text, capture_output=True,
                text=True, encoding="utf-8", errors="replace", timeout=timeout,
                creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0,
            )
        except subprocess.TimeoutExpired as error:
            raise BenchmarkError(f"ADB 命令超时（{timeout:g} 秒） {' '.join(arguments[:3])}") from error
        except OSError as error:
            raise BenchmarkError(f"无法执行 ADB {error}") from error
        output = result.stdout + result.stderr
        if check and result.returncode:
            raise BenchmarkError(f"ADB 命令失败 {' '.join(arguments[:3])}\n{output.strip()}")
        return output


def find_adb() -> str:
    found = shutil.which("adb")
    if found:
        return found
    sdk_candidates = [os.environ.get("ANDROID_HOME"), os.environ.get("ANDROID_SDK_ROOT")]
    if os.environ.get("LOCALAPPDATA"):
        sdk_candidates.append(str(Path(os.environ["LOCALAPPDATA"]) / "Android" / "Sdk"))
    executable = "adb.exe" if os.name == "nt" else "adb"
    for sdk in sdk_candidates:
        if sdk:
            candidate = Path(sdk) / "platform-tools" / executable
            if candidate.is_file():
                return str(candidate)
    raise BenchmarkError("找不到 adb，请安装 Android SDK Platform Tools 或设置 ANDROID_HOME。")


def select_device(output: str, requested: str | None = None) -> str:
    devices = {}
    for line in output.splitlines():
        parts = line.split()
        if len(parts) >= 2 and not line.startswith(("List of devices", "*")):
            devices[parts[0]] = parts[1]
    if requested:
        if devices.get(requested) != "device":
            raise BenchmarkError(f"设备 {requested} 当前不可用，请检查连接与调试授权。")
        return requested
    ready = [serial for serial, state in devices.items() if state == "device"]
    if not ready:
        raise BenchmarkError("没有可用 ADB 设备，请连接已授权 USB 调试的手机。")
    if len(ready) > 1:
        raise BenchmarkError("检测到多个设备，请通过 -Serial 指定目标。 " + ", ".join(ready))
    return ready[0]


def select_apk(abi_list: str, repository: Path = REPO_ROOT) -> Path:
    supported = abi_list.strip().split(",")
    # An x86 emulator can advertise ARM translation; select its native ABI first.
    abi = "x86_64" if "x86_64" in supported else "arm64-v8a" if "arm64-v8a" in supported else None
    if abi is None:
        raise BenchmarkError(f"设备 ABI 暂不受支持 {abi_list.strip()}")
    apk = repository / "dist" / f"gpui-benchmark-{abi}-release.apk"
    if not apk.is_file():
        raise BenchmarkError(f"缺少安装包 {apk}。请先运行构建入口生成 {abi} Release 包。")
    return apk


def prepare_output(directory: Path) -> Path:
    directory = directory.absolute()
    if directory.is_symlink() or any(parent.is_symlink() for parent in directory.parents):
        raise BenchmarkError("输出目录含符号链接，请使用实际目录。")
    marker = directory / "owner.json"
    if directory.exists() and any(directory.iterdir()):
        try:
            owner = json.loads(marker.read_text(encoding="utf-8"))
        except (OSError, ValueError) as error:
            raise BenchmarkError("输出目录已有其他文件，请选择空目录或本工具已有输出目录。") from error
        if owner.get("owner") != "gpui-benchmark-adb-v1":
            raise BenchmarkError("输出目录归属与本工具不符。")
        for entry in directory.iterdir():
            if entry.name not in OWNED_FILES or not entry.is_file() or entry.is_symlink():
                raise BenchmarkError(f"输出目录存在非工具文件，请先移出 {entry.name}")
        for entry in directory.iterdir():
            entry.unlink()
    directory.mkdir(parents=True, exist_ok=True)
    write_json(marker, {"owner": "gpui-benchmark-adb-v1", "budget_bytes": OUTPUT_BUDGET})
    return directory


def write_json(path: Path, value: object) -> None:
    path.write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")


def validate_report(report: object, run_id: str) -> dict:
    if not isinstance(report, dict) or report.get("run_id") != run_id:
        raise BenchmarkError("结果 run_id 与本轮测试不符，已拒绝沿用旧成绩。")
    if report.get("schema_version") != 2 or report.get("protocol_id") != "gpui-benchmark-v1":
        raise BenchmarkError("App 结果协议版本不受支持，请安装本仓库的当前 Release 包。")
    if not isinstance(report.get("results"), list):
        raise BenchmarkError("App 结果缺少逐场景统计。")
    if report.get("status") not in {"running", "completed", "cancelled", "invalid"}:
        raise BenchmarkError("App 结果状态无效。")
    if report["status"] == "completed":
        repetitions = report.get("options", {}).get("repetitions", 0)
        cases = report["results"]
        identifiers = [case.get("case_id") for case in cases if isinstance(case, dict)]
        if not isinstance(repetitions, int) or not 1 <= repetitions <= 5:
            raise BenchmarkError("App 结果缺少有效的重复次数。")
        if len(cases) != repetitions * 10 or len(set(identifiers)) != len(cases):
            raise BenchmarkError("App 报告场景数量或唯一标识不完整。")
    return report


def parse_surface_layers(raw: str, package: str = PACKAGE) -> list[dict]:
    layers = []
    for block in re.split(r"(?m)(?=^\s*layerName\s*=)", raw):
        name_match = re.search(r"(?m)^\s*layerName\s*=\s*(.+)$", block)
        if not name_match:
            continue
        name = name_match.group(1).strip()
        if package not in name or "splash" in name.lower():
            continue
        histogram_match = re.search(
            r"(?:presentToPresent|present2present)\s+histogram[^\n]*\n([^\n]*)", block, re.I
        )
        histogram = {}
        if histogram_match:
            histogram = {int(ms): int(count) for ms, count in re.findall(
                r"(\d+)ms\s*=\s*(\d+)", histogram_match.group(1)
            )}
        frames = re.search(r"totalFrames\s*=\s*(\d+)", block)
        fps = re.search(r"averageFPS\s*=\s*([\d.]+)", block)
        layers.append({
            "layer_name": name,
            "total_frames": int(frames.group(1)) if frames else 0,
            "average_fps_reported": float(fps.group(1)) if fps else None,
            "present_to_present_ms": histogram,
        })
    return layers


def surface_delta(before: str, after: str) -> list[dict]:
    previous = {layer["layer_name"]: layer for layer in parse_surface_layers(before)}
    results = []
    for layer in parse_surface_layers(after):
        baseline = previous.get(layer["layer_name"], {})
        old_histogram = baseline.get("present_to_present_ms", {})
        histogram = {ms: count - old_histogram.get(ms, 0)
                     for ms, count in layer["present_to_present_ms"].items()}
        frames = layer["total_frames"] - baseline.get("total_frames", 0)
        if frames < 0 or any(count < 0 for count in histogram.values()):
            results.append({"layer_name": layer["layer_name"], "status": "counters_reset"})
            continue
        count = sum(histogram.values())
        total_ms = sum(ms * occurrences for ms, occurrences in histogram.items())
        results.append({
            "layer_name": layer["layer_name"],
            "status": "available" if count and total_ms else "no_present_samples",
            "total_frames_delta": frames,
            "present_to_present_samples": count,
            "average_fps_from_histogram": 1000 * count / total_ms if total_ms else None,
            "present_to_present_ms": histogram,
        })
    return results


class EventStream:
    def __init__(self, adb: Adb, path: Path):
        self.process = subprocess.Popen(
            adb.command("logcat", "-v", "raw", "-T", "1", "-s", "GpuiBenchmark:I", "*:S"),
            stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True,
            encoding="utf-8", errors="replace", bufsize=1,
            creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0,
        )
        self.events: queue.Queue[str] = queue.Queue(maxsize=256)
        self.path = path
        self.truncated = False
        self.dropped_events = 0
        self.worker = threading.Thread(target=self._read, daemon=True)
        self.worker.start()

    def _read(self) -> None:
        written = 0
        with self.path.open("w", encoding="utf-8") as output:
            for line in self.process.stdout:
                encoded_size = len(line.encode("utf-8"))
                if written + encoded_size <= LOG_BUDGET:
                    output.write(line)
                    output.flush()
                    written += encoded_size
                else:
                    self.truncated = True
                try:
                    self.events.put_nowait(line.strip())
                except queue.Full:
                    self.dropped_events += 1

    def close(self) -> None:
        if self.process.poll() is None:
            self.process.terminate()
            try:
                self.process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait(timeout=5)
        self.worker.join(timeout=5)
        if self.process.stdout:
            self.process.stdout.close()


class SystemCapture:
    def __init__(self, adb: Adb, directory: Path, run_id: str, duration: int):
        self.adb = adb
        self.directory = directory
        self.remote_trace = f"/data/misc/perfetto-traces/gpui-benchmark-{run_id}.pftrace"
        self.duration = duration
        self.trace_pid: str | None = None
        self.surface_owned = False
        self.surface_before = ""
        self.coverage = {
            "scope": "whole_capture_including_launch_and_warmup",
            "per_case_system_frames": False,
            "surfaceflinger": {"status": "unavailable"},
            "perfetto": {
                "status": "unavailable", "buffer_mib": 128,
                "retention": "ring_buffer_may_only_retain_end_of_run",
                "app_frame_timeline_verified": False,
            },
            "cleanup_errors": [],
        }

    def start(self) -> None:
        try:
            self.surface_before = self.adb.run("shell", "dumpsys", "SurfaceFlinger", "--timestats", "-dump")
            (self.directory / "surfaceflinger-before.txt").write_text(self.surface_before, encoding="utf-8")
            if re.search(r"permission denial|unknown option|not found", self.surface_before, re.I):
                raise BenchmarkError("设备未开放 SurfaceFlinger timestats。")
            if not self.surface_before.strip():
                self.adb.run("shell", "dumpsys", "SurfaceFlinger", "--timestats", "-enable")
                self.surface_owned = True
            self.coverage["surfaceflinger"] = {
                "status": "collecting", "owned_enable": self.surface_owned,
                "baseline_policy": "delta_without_clearing_existing_statistics",
            }
        except BenchmarkError as error:
            self.coverage["surfaceflinger"]["reason"] = str(error)
        try:
            capabilities = self.adb.run("shell", "perfetto", "--query", timeout=15)
            advertised = "android.surfaceflinger.frametimeline" in capabilities
            config = perfetto_config(self.duration, advertised)
            started = self.adb.run(
                "shell", "perfetto", "--background-wait", "--txt", "-c", "-",
                "-o", self.remote_trace, input_text=config, timeout=25,
            )
            (self.directory / "perfetto-start.txt").write_text(started, encoding="utf-8")
            matches = re.findall(r"(?m)^\s*(\d+)\s*$", started)
            if not matches:
                matches = re.findall(r"\bpid\s*[:=]?\s*(\d+)\b", started, re.I)
            if not matches:
                raise BenchmarkError("Perfetto 未返回本次捕获的进程号。")
            self.trace_pid = matches[-1]
            self.coverage["perfetto"].update({
                "status": "collecting", "frametimeline_advertised": advertised,
                "duration_limit_seconds": self.duration,
            })
        except BenchmarkError as error:
            self.coverage["perfetto"]["reason"] = str(error)

    def finish(self) -> None:
        if self.coverage["surfaceflinger"]["status"] == "collecting":
            try:
                after = self.adb.run("shell", "dumpsys", "SurfaceFlinger", "--timestats", "-dump")
                (self.directory / "surfaceflinger-after.txt").write_text(after, encoding="utf-8")
                layers = surface_delta(self.surface_before, after)
                available = any(layer["status"] == "available" for layer in layers)
                self.coverage["surfaceflinger"].update({
                    "status": "available" if available else "no_app_present_samples",
                    "layers": layers,
                    "histogram_resolution": "Android bucketed milliseconds",
                })
            except BenchmarkError as error:
                self.coverage["surfaceflinger"].update({"status": "unavailable", "reason": str(error)})
        self.stop_trace()
        if self.trace_pid:
            try:
                pull_bounded(self.adb, self.remote_trace, self.directory / "trace.pftrace")
                self.coverage["perfetto"]["status"] = "captured_unanalyzed"
                analyze_trace(self.directory, self.coverage["perfetto"])
            except BenchmarkError as error:
                self.coverage["perfetto"].update({"status": "unavailable", "reason": str(error)})

    def stop_trace(self) -> None:
        if not self.trace_pid:
            return
        try:
            command_line = self.adb.run("shell", "cat", f"/proc/{self.trace_pid}/cmdline", check=False)
            if "perfetto" in command_line and self.remote_trace in command_line:
                self.adb.run("shell", "kill", "-INT", self.trace_pid, check=False)
                time.sleep(1)
        except BenchmarkError as error:
            self.coverage["cleanup_errors"].append(str(error))

    def close(self) -> None:
        self.stop_trace()
        if self.surface_owned:
            try:
                self.adb.run("shell", "dumpsys", "SurfaceFlinger", "--timestats", "-disable")
                self.surface_owned = False
            except BenchmarkError as error:
                self.coverage["cleanup_errors"].append(str(error))
        if self.trace_pid:
            try:
                self.adb.run("shell", "rm", "-f", self.remote_trace, check=False)
            except BenchmarkError as error:
                self.coverage["cleanup_errors"].append(str(error))


def perfetto_config(duration: int, frame_timeline: bool) -> str:
    frame_source = 'data_sources { config { name: "android.surfaceflinger.frametimeline" } }' if frame_timeline else ""
    return f'''duration_ms: {duration * 1000}
buffers {{ size_kb: {TRACE_BUFFER_KIB} fill_policy: RING_BUFFER }}
data_sources {{
  config {{
    name: "linux.ftrace"
    ftrace_config {{
      ftrace_events: "sched/sched_switch"
      ftrace_events: "sched/sched_waking"
      ftrace_events: "power/cpu_frequency"
      ftrace_events: "power/cpu_idle"
      atrace_categories: "gfx"
      atrace_categories: "view"
      atrace_categories: "am"
      atrace_apps: "{PACKAGE}"
    }}
  }}
}}
{frame_source}
'''


def pull_bounded(adb: Adb, remote: str, local: Path) -> None:
    size_text = adb.run("shell", "stat", "-c", "%s", remote).strip()
    if not size_text.isdigit() or int(size_text) <= 0:
        raise BenchmarkError(f"设备产物缺失或为空 {remote}")
    used = sum(path.stat().st_size for path in local.parent.iterdir() if path.is_file())
    if used + int(size_text) > OUTPUT_BUDGET:
        raise BenchmarkError("本轮产物超出 256 MiB 预算，已保留现有结果。")
    adb.run("pull", remote, str(local), timeout=90)
    if not local.is_file() or local.stat().st_size != int(size_text):
        raise BenchmarkError(f"设备产物未完整拉取 {remote}")


def analyze_trace(directory: Path, coverage: dict) -> None:
    processor = shutil.which("trace_processor_shell")
    if not processor:
        coverage["analysis_reason"] = "trace_processor_shell_unavailable"
        return
    sql = f"""SELECT COUNT(*) AS app_frame_timeline_samples
FROM actual_frame_timeline_slice
WHERE layer_name LIKE '%{PACKAGE}%';"""
    try:
        result = subprocess.run(
            [processor, "--csv", "-q", sql, str(directory / "trace.pftrace")],
            capture_output=True, text=True, encoding="utf-8", errors="replace", timeout=60,
            creationflags=subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0,
        )
        (directory / "trace-analysis.csv").write_text(result.stdout + result.stderr, encoding="utf-8")
        if result.returncode:
            coverage["analysis_reason"] = "frame_timeline_query_failed"
            return
        lines = result.stdout.strip().splitlines()
        samples = int(lines[-1].strip().strip('"'))
        coverage.update({
            "status": "analyzed", "app_frame_timeline_samples": samples,
            "app_frame_timeline_verified": samples > 0,
        })
    except (subprocess.TimeoutExpired, ValueError, IndexError, OSError) as error:
        coverage["analysis_reason"] = str(error)


def collect_device(adb: Adb, directory: Path) -> dict:
    properties = {
        "manufacturer": "ro.product.manufacturer", "model": "ro.product.model",
        "android_release": "ro.build.version.release", "sdk": "ro.build.version.sdk",
        "abi_list": "ro.product.cpu.abilist", "emulator": "ro.kernel.qemu",
    }
    device = {key: adb.run("shell", "getprop", prop).strip() for key, prop in properties.items()}
    write_json(directory / "device.json", device)
    (directory / "display.txt").write_text(adb.run("shell", "dumpsys", "display"), encoding="utf-8")
    return device


def collect_snapshot(adb: Adb, directory: Path, suffix: str) -> None:
    for name, command in [
        ("meminfo", ("shell", "dumpsys", "meminfo", PACKAGE)),
        ("thermal", ("shell", "dumpsys", "thermalservice")),
    ]:
        try:
            text = adb.run(*command, timeout=15)
        except BenchmarkError as error:
            text = f"Unavailable\n{error}\n"
        (directory / f"{name}-{suffix}.txt").write_text(text, encoding="utf-8")


def wait_for_completion(adb: Adb, events: EventStream, run_id: str, duration: int) -> None:
    deadline = time.monotonic() + duration
    next_health_check = time.monotonic() + 10
    while time.monotonic() < deadline:
        try:
            line = events.events.get(timeout=1)
        except queue.Empty:
            line = ""
        if line == f"BENCHMARK_COMPLETE {run_id}":
            return
        if line.startswith("BENCHMARK_EVENT "):
            print(line, flush=True)
        if events.process.poll() is not None:
            raise BenchmarkError("ADB 日志通道提前结束，请检查设备连接。")
        if time.monotonic() >= next_health_check:
            if not adb.run("shell", "pidof", PACKAGE, timeout=5, check=False).strip().isdigit():
                raise BenchmarkError("Benchmark App 已退出，请查看应用崩溃日志。")
            next_health_check = time.monotonic() + 10
    raise BenchmarkError("Benchmark 超出预计运行时间，已停止本轮采集。")


def render_html(report: dict, coverage: dict, device: dict) -> str:
    escape = lambda value: html.escape(str(value))
    rows = []
    for case in report.get("results", []):
        frames = case.get("frame_stats", {})
        values = [case.get("case_id", ""), f'{frames.get("fps", 0):.1f}',
                  f'{frames.get("p95_ms", 0):.2f}', f'{frames.get("p99_ms", 0):.2f}',
                  f'{frames.get("max_ms", 0):.2f}', f'{case.get("applied_updates", 0):,}',
                  f'{case.get("dropped_updates", 0):,}']
        rows.append("<tr>" + "".join(f"<td>{escape(value)}</td>" for value in values) + "</tr>")
    layers = coverage.get("surfaceflinger", {}).get("layers", [])
    surfaces = "".join(
        f'<li>{escape(layer["layer_name"])} · {escape(layer["status"])}'
        + (f' · {layer["average_fps_from_histogram"]:.1f} 次呈现/秒' if layer.get("average_fps_from_histogram") else "")
        + "</li>" for layer in layers
    ) or "<li>本轮没有可验证的 App 呈现样本</li>"
    embedded = json.dumps({"app": report, "system": coverage, "device": device}, ensure_ascii=False).replace("<", "\\u003c")
    return f'''<!doctype html><html lang="zh-CN"><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1"><title>GPUI benchmark 结果</title>
<style>
:root{{color-scheme:dark;font-family:Roboto,system-ui,sans-serif;background:#080d14;color:#f2f7fa}}
*{{box-sizing:border-box}}body{{margin:0;padding:clamp(20px,4vw,64px)}}main{{max-width:1180px;margin:auto}}
header{{border-bottom:1px solid #253b4c;padding-bottom:32px}}h1{{font-size:clamp(32px,6vw,64px);margin:16px 0}}
.muted,p,small{{color:#91a9bb}}.score{{font-size:clamp(68px,12vw,140px);font-weight:900;line-height:1;color:#c8f26b}}
.meta{{display:flex;flex-wrap:wrap;gap:24px;align-items:baseline}}section{{margin-top:36px}}h2{{font-size:20px}}
.grid{{display:grid;grid-template-columns:repeat(auto-fit,minmax(min(280px,100%),1fr));gap:16px}}
.panel{{background:#101b29;padding:24px}}.value{{font-size:24px;color:#55e4d4;margin:8px 0}}
.table-scroll{{overflow:auto;border:1px solid #253b4c}}table{{border-collapse:collapse;width:100%;font-variant-numeric:tabular-nums}}
td,th{{text-align:right;padding:14px 18px;white-space:nowrap;border-bottom:1px solid #253b4c}}td:first-child,th:first-child{{text-align:left}}
th{{font-weight:500;color:#91a9bb}}a{{color:#55e4d4}}li{{margin:10px 0;overflow-wrap:anywhere}}code{{font-family:monospace}}
</style><main><header><small>GPUI benchmark · {escape(report.get("protocol_id", ""))}</small>
<h1>{escape(device.get("manufacturer", ""))} {escape(device.get("model", "设备结果"))}</h1>
<div class="meta"><div><div class="score">{report.get("score", 0):.0f}</div><p>App 内回调综合得分</p></div>
<p>{escape(report.get("status", "unknown"))} · {escape(report.get("mode", ""))}<br>
Android {escape(device.get("android_release", ""))} · {escape(device.get("abi_list", ""))}</p></div></header>
<section class="grid"><div class="panel"><small>App 统计</small><div class="value">{len(rows)} 个场景结果</div>
<p>各场景使用整个正式采样段，预热样本已排除。数值来自 CPU 侧帧回调间隔。</p></div>
<div class="panel"><small>SurfaceFlinger</small><div class="value">{escape(coverage.get("surfaceflinger", {}).get("status", "unavailable"))}</div>
<p>系统侧呈现统计覆盖整个采集窗口，包含启动与预热。使用前后计数差，按应用图层筛选。</p></div>
<div class="panel"><small>Perfetto</small><div class="value">{escape(coverage.get("perfetto", {}).get("status", "unavailable"))}</div>
<p>128 MiB 环形缓冲区可能仅保留测试末段。App 帧时间线覆盖状态以原始数据和分析结果为准。</p></div></section>
<section><h2>逐场景回调统计</h2><div class="table-scroll"><table><thead><tr>
<th>场景</th><th>回调 / 秒</th><th>P95 ms</th><th>P99 ms</th><th>最大 ms</th><th>已应用</th><th>丢弃</th>
</tr></thead><tbody>{''.join(rows)}</tbody></table></div></section>
<section><h2>系统侧呈现</h2><ul>{surfaces}</ul></section>
<section><h2>原始证据</h2><p><a href="app-report.json">App JSON</a> · <a href="system-coverage.json">系统覆盖记录</a> ·
<a href="trace.pftrace">Perfetto trace</a> · <a href="events.log">测试事件</a></p>
<p>设备刷新模式、温度和内存快照保存在同一目录。系统数据缺失时保留缺失状态。</p></section>
<script id="benchmark-data" type="application/json">{embedded}</script></main></html>'''


def run_benchmark(adb: Adb, directory: Path, quick: bool = False) -> dict:
    directory = prepare_output(directory)
    run_id = str(uuid.uuid4())
    warmup, measurement, repetitions = (1000, 3000, 1) if quick else (3000, 20000, 3)
    nominal_seconds = (warmup + measurement) * 10 * repetitions // 1000
    device = collect_device(adb, directory)
    apk = select_apk(device["abi_list"])
    adb.run("install", "-r", str(apk), timeout=120)
    collect_snapshot(adb, directory, "before")
    capture = SystemCapture(adb, directory, run_id, nominal_seconds + 60)
    events = None
    success = False
    try:
        capture.start()
        events = EventStream(adb, directory / "events.log")
        adb.run(
            "shell", "am", "start", "-W", "-S", "-n", ACTIVITY,
            "--ez", "benchmark", "true", "--es", "benchmark_run_id", run_id,
            "--ei", "benchmark_warmup_ms", str(warmup),
            "--ei", "benchmark_measurement_ms", str(measurement),
            "--ei", "benchmark_repetitions", str(repetitions), timeout=45,
        )
        print(f"自动测试已启动，预计 {nominal_seconds} 秒。run_id={run_id}", flush=True)
        wait_for_completion(adb, events, run_id, nominal_seconds + 120)
        pull_bounded(adb, REMOTE_REPORT, directory / "app-report.json")
        report = validate_report(json.loads((directory / "app-report.json").read_text(encoding="utf-8")), run_id)
        capture.finish()
        collect_snapshot(adb, directory, "after")
        capture.coverage["event_log"] = {"truncated": events.truncated, "queue_dropped": events.dropped_events}
        success = True
    except BaseException as error:
        (directory / "failure.txt").write_text(
            f"{type(error).__name__}\n{str(error)[:16000]}\n", encoding="utf-8"
        )
        raise
    finally:
        if events:
            events.close()
        capture.close()
        write_json(directory / "system-coverage.json", capture.coverage)
        if not success:
            try:
                adb.run("shell", "am", "force-stop", PACKAGE, timeout=10, check=False)
            except BenchmarkError as error:
                capture.coverage["cleanup_errors"].append(str(error))
                write_json(directory / "system-coverage.json", capture.coverage)
    summary = {"run_id": run_id, "app_status": report["status"], "system": capture.coverage, "device": device}
    write_json(directory / "summary.json", summary)
    (directory / "report.html").write_text(render_html(report, capture.coverage, device), encoding="utf-8")
    print(f"报告已生成 {directory / 'report.html'}", flush=True)
    return report


def main(arguments: list[str] | None = None) -> int:
    if os.name == "nt":
        sys.stdout.reconfigure(encoding="utf-8")
        sys.stderr.reconfigure(encoding="utf-8")
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--serial")
    parser.add_argument("--quick", action="store_true", help="缩短的非排名回归采样")
    parser.add_argument("--output-directory", type=Path, default=REPO_ROOT / ".build" / "benchmark-results" / "latest")
    options = parser.parse_args(arguments)
    try:
        executable = find_adb()
        controller = Adb(executable)
        serial = select_device(controller.run("devices", "-l"), options.serial)
        report = run_benchmark(Adb(executable, serial), options.output_directory, options.quick)
        return 0 if report.get("status") == "completed" else 2
    except (BenchmarkError, OSError, ValueError) as error:
        print(str(error), file=sys.stderr)
        return 1
    except KeyboardInterrupt:
        print("测试已取消，采集资源已收尾。", file=sys.stderr)
        return 130


if __name__ == "__main__":
    raise SystemExit(main())
