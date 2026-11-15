package dev.gpui.lab;

import android.app.Activity;
import android.content.Intent;
import android.os.Build;
import android.os.SystemClock;
import android.os.Trace;
import android.util.Log;
import android.view.Display;
import android.net.ConnectivityManager;
import android.net.Network;
import android.net.NetworkCapabilities;
import org.json.JSONException;
import org.json.JSONObject;
import java.io.File;
import java.io.FileOutputStream;
import java.io.IOException;
import java.net.HttpURLConnection;
import java.net.URL;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.time.Instant;
import java.util.Arrays;
import java.util.Comparator;
import java.util.UUID;
import java.util.concurrent.RejectedExecutionException;
import java.util.concurrent.ScheduledThreadPoolExecutor;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicBoolean;

/** Android metadata, durable reports and HTTPS upload run outside GPUI's frame loop. */
final class BenchmarkHost {
    static final String LEADERBOARD_URL = "https://visionguard.xgwnje.cn/gpui-benchmark/";
    private static final String TAG = "GpuiBenchmark";
    private final Activity activity;
    private final ScheduledThreadPoolExecutor executor = new ScheduledThreadPoolExecutor(1);
    private volatile String uploadStatus = "";
    private JSONObject benchmarkDevice;
    private int previousDisplayMode;
    private float previousRefreshRate;
    private boolean previousKeepAwake;
    private boolean displayPrepared;
    private volatile boolean benchmarkActive;
    private volatile HttpURLConnection activeConnection;
    private int previousOrientation;
    private int activeTraceCase = -1;
    private final AtomicBoolean retryQueued = new AtomicBoolean();
    private final AtomicBoolean retryScheduled = new AtomicBoolean();
    private ConnectivityManager connectivity;
    private final ConnectivityManager.NetworkCallback networkCallback = new ConnectivityManager.NetworkCallback() {
        @Override public void onCapabilitiesChanged(Network network, NetworkCapabilities capabilities) {
            if (capabilities.hasCapability(NetworkCapabilities.NET_CAPABILITY_VALIDATED)) retryPending();
        }
    };

    BenchmarkHost(Activity activity) {
        this.activity = activity;
        executor.setExecuteExistingDelayedTasksAfterShutdownPolicy(false);
        executor.setRemoveOnCancelPolicy(true);
    }
    String status() { return uploadStatus; }

    void watchNetwork() {
        connectivity = activity.getSystemService(ConnectivityManager.class);
        if (connectivity != null) connectivity.registerDefaultNetworkCallback(networkCallback);
        retryPending();
    }

    JSONObject prepare() {
        benchmarkActive = true;
        HttpURLConnection uploading = activeConnection;
        if (uploading != null) uploading.disconnect();
        Display display = activity.getWindowManager().getDefaultDisplay();
        Display.Mode selected = display.getMode();
        for (Display.Mode candidate : display.getSupportedModes()) {
            if (candidate.getPhysicalWidth() == selected.getPhysicalWidth()
                    && candidate.getPhysicalHeight() == selected.getPhysicalHeight()
                    && candidate.getRefreshRate() > selected.getRefreshRate()) selected = candidate;
        }
        android.view.WindowManager.LayoutParams previous = activity.getWindow().getAttributes();
        previousDisplayMode = previous.preferredDisplayModeId;
        previousRefreshRate = previous.preferredRefreshRate;
        previousKeepAwake = (previous.flags & android.view.WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON) != 0;
        displayPrepared = true;
        previousOrientation = activity.getRequestedOrientation();
        final Display.Mode requested = selected;
        activity.runOnUiThread(() -> {
            android.view.WindowManager.LayoutParams attributes = activity.getWindow().getAttributes();
            attributes.preferredDisplayModeId = requested.getModeId();
            attributes.preferredRefreshRate = requested.getRefreshRate();
            activity.getWindow().setAttributes(attributes);
            activity.getWindow().addFlags(android.view.WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON);
            activity.setRequestedOrientation(android.content.pm.ActivityInfo.SCREEN_ORIENTATION_PORTRAIT);
        });
        benchmarkDevice = deviceInfo();
        try {
            benchmarkDevice.put("refresh_hz", (double) selected.getRefreshRate());
            benchmarkDevice.put("requested_refresh_hz", (double) selected.getRefreshRate());
            benchmarkDevice.put("run_id", UUID.randomUUID().toString());
        } catch (JSONException error) { throw new IllegalStateException(error); }
        return benchmarkDevice;
    }

    JSONObject confirmDevice() {
        JSONObject actual = deviceInfo();
        try {
            actual.put("run_id", benchmarkDevice.getString("run_id"));
            actual.put("requested_refresh_hz", benchmarkDevice.getDouble("requested_refresh_hz"));
        } catch (JSONException error) { throw new IllegalStateException(error); }
        benchmarkDevice = actual;
        return actual;
    }

    synchronized void finish() {
        benchmarkActive = false;
        closeActiveTrace();
        if (!displayPrepared) return;
        displayPrepared = false;
        final int originalMode = previousDisplayMode;
        final float originalRate = previousRefreshRate;
        final boolean keepAwake = previousKeepAwake;
        final int orientation = previousOrientation;
        activity.runOnUiThread(() -> {
            android.view.WindowManager.LayoutParams attributes = activity.getWindow().getAttributes();
            attributes.preferredDisplayModeId = originalMode;
            attributes.preferredRefreshRate = originalRate;
            activity.getWindow().setAttributes(attributes);
            if (!keepAwake) activity.getWindow().clearFlags(android.view.WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON);
            activity.setRequestedOrientation(orientation);
        });
    }

    String settings() {
        Intent intent = activity.getIntent();
        if (!intent.getBooleanExtra("benchmark", false)) return "";
        JSONObject result = new JSONObject();
        try {
            result.put("run_id", intent.getStringExtra("benchmark_run_id") == null
                    ? UUID.randomUUID().toString() : intent.getStringExtra("benchmark_run_id"));
            result.put("warmup_ms", intent.getIntExtra("benchmark_warmup_ms", 3000));
            result.put("measurement_ms", intent.getIntExtra("benchmark_measurement_ms", 20000));
            result.put("repetitions", intent.getIntExtra("benchmark_repetitions", 3));
        } catch (JSONException error) {
            throw new IllegalStateException(error);
        }
        return result.toString();
    }

    JSONObject deviceInfo() {
        JSONObject result = new JSONObject();
        Display display = activity.getWindowManager().getDefaultDisplay();
        Display.Mode mode = display.getMode();
        try {
            result.put("manufacturer", Build.MANUFACTURER);
            result.put("model", Build.MODEL);
            result.put("android", Build.VERSION.RELEASE);
            result.put("sdk", Build.VERSION.SDK_INT);
            result.put("abi", Build.SUPPORTED_ABIS[0]);
            result.put("refresh_hz", (double) mode.getRefreshRate());
            result.put("width_px", mode.getPhysicalWidth());
            result.put("height_px", mode.getPhysicalHeight());
            result.put("is_emulator", Build.FINGERPRINT.startsWith("generic")
                    || Build.MODEL.contains("sdk_gphone") || Build.HARDWARE.contains("ranchu")
                    || Build.HARDWARE.contains("goldfish"));
        } catch (JSONException error) {
            throw new IllegalStateException(error);
        }
        return result;
    }

    synchronized void marker(String marker, int caseIndex) {
        Log.i(TAG, "BENCHMARK_EVENT " + marker + " " + caseIndex
                + " monotonic_ns=" + SystemClock.elapsedRealtimeNanos());
        if (Build.VERSION.SDK_INT >= 29) {
            if ("measuring".equals(marker)) {
                closeActiveTrace();
                Trace.beginAsyncSection("GPUI benchmark measure", caseIndex);
                activeTraceCase = caseIndex;
            }
            if ("case_complete".equals(marker) && activeTraceCase == caseIndex) closeActiveTrace();
        }
    }

    private void closeActiveTrace() {
        if (Build.VERSION.SDK_INT >= 29 && activeTraceCase >= 0) Trace.endAsyncSection("GPUI benchmark measure", activeTraceCase);
        activeTraceCase = -1;
    }

    void saveAndUpload(String rawReport) {
        // Capture display metadata on the calling thread before any later mode switch.
        JSONObject device = benchmarkDevice == null ? deviceInfo() : benchmarkDevice;
        executor.execute(() -> {
            try {
                JSONObject report = new JSONObject(rawReport);
                report.put("device", device);
                report.put("app_version", "0.2.0");
                report.put("generated_at", Instant.now().toString());
                File directory = reportDirectory();
                String runId = report.getString("run_id");
                if (!runId.matches("[A-Za-z0-9_-]{1,80}")) throw new IOException("Invalid run ID");
                byte[] payload = report.toString().getBytes(StandardCharsets.UTF_8);
                File pending = "completed".equals(report.optString("status"))
                        && "standard".equals(report.optString("mode"))
                        && !device.optBoolean("is_emulator")
                        ? new File(directory, runId + ".pending.json") : null;
                if (pending != null) atomicWrite(pending, payload);
                atomicWrite(new File(directory, "latest.json"), payload);
                Log.i(TAG, "BENCHMARK_COMPLETE " + runId);
                if (pending != null) {
                    upload(pending);
                    retryPending();
                } else {
                    updateStatus("报告已保存 · 本次未进入标准机型排名");
                }
            } catch (IOException | JSONException error) {
                Log.e(TAG, "Could not save benchmark report", error);
                updateStatus("报告保存失败 · " + error.getMessage());
            }
        });
    }

    void retryPending() {
        if (benchmarkActive || executor.isShutdown() || !retryQueued.compareAndSet(false, true)) return;
        try {
            executor.execute(() -> {
                try {
                    File[] pending = reportDirectory().listFiles((dir, name) -> name.endsWith(".pending.json"));
                    if (pending == null) return;
                    Arrays.sort(pending, Comparator.comparingLong(File::lastModified));
                    for (File report : pending) {
                        if (Thread.currentThread().isInterrupted() || benchmarkActive) return;
                        upload(report);
                    }
                } catch (IOException error) {
                    Log.e(TAG, "Could not read pending benchmark reports", error);
                    updateStatus("待上传报告读取失败");
                } finally {
                    retryQueued.set(false);
                }
            });
        } catch (RejectedExecutionException error) {
            retryQueued.set(false);
        }
    }

    private void upload(File pending) {
        if (benchmarkActive) return;
        HttpURLConnection connection = null;
        try {
            updateStatus("正在上传成绩");
            byte[] payload = Files.readAllBytes(pending.toPath());
            connection = (HttpURLConnection) new URL(LEADERBOARD_URL + "api/runs").openConnection();
            activeConnection = connection;
            connection.setConnectTimeout(10000);
            connection.setReadTimeout(15000);
            connection.setRequestMethod("POST");
            connection.setRequestProperty("Content-Type", "application/json; charset=utf-8");
            connection.setFixedLengthStreamingMode(payload.length);
            connection.setDoOutput(true);
            try (java.io.OutputStream output = connection.getOutputStream()) { output.write(payload); }
            int code = connection.getResponseCode();
            java.io.InputStream responseStream = code >= 200 && code < 300
                    ? connection.getInputStream() : connection.getErrorStream();
            if (responseStream == null) throw new IOException("HTTP " + code);
            byte[] response;
            try (java.io.InputStream input = responseStream) { response = readResponse(input); }
            JSONObject acknowledgement = new JSONObject(new String(response, StandardCharsets.UTF_8));
            if (code < 200 || code >= 300) {
                throw new IOException("HTTP " + code + " " + acknowledgement.optString("error"));
            }
            atomicWrite(new File(pending.getParentFile(), "latest-upload.json"), response);
            if (!pending.delete()) Log.w(TAG, "Uploaded report remains queued for an idempotent retry");
            if (!acknowledgement.isNull("rank") && acknowledgement.has("rank")) {
                updateStatus("已上传 · 提交时机型第 " + acknowledgement.getInt("rank") + " 名");
            } else {
                updateStatus("已上传 · 详情见排行榜");
            }
        } catch (IOException | JSONException error) {
            Log.w(TAG, "Benchmark upload pending", error);
            updateStatus("已保存 · 等待网络后自动补传");
            scheduleRetry();
        } finally {
            if (connection != null) connection.disconnect();
            if (activeConnection == connection) activeConnection = null;
        }
    }

    private void scheduleRetry() {
        if (executor.isShutdown() || !retryScheduled.compareAndSet(false, true)) return;
        try {
            executor.schedule(() -> {
                retryScheduled.set(false);
                retryPending();
            }, 60, TimeUnit.SECONDS);
        } catch (RejectedExecutionException error) {
            retryScheduled.set(false);
        }
    }

    private static byte[] readResponse(java.io.InputStream input) throws IOException {
        java.io.ByteArrayOutputStream output = new java.io.ByteArrayOutputStream();
        byte[] buffer = new byte[4096];
        for (int count; (count = input.read(buffer)) != -1;) {
            if (output.size() + count > 65536) throw new IOException("Response exceeds 64 KiB");
            output.write(buffer, 0, count);
        }
        return output.toByteArray();
    }

    private File reportDirectory() throws IOException {
        File external = activity.getExternalFilesDir(null);
        File directory = new File(external == null ? activity.getFilesDir() : external, "benchmark");
        if (!directory.isDirectory() && !directory.mkdirs()) throw new IOException("Report directory unavailable");
        return directory;
    }

    private static void atomicWrite(File destination, byte[] payload) throws IOException {
        File temporary = new File(destination.getPath() + ".tmp");
        try (FileOutputStream output = new FileOutputStream(temporary)) {
            output.write(payload);
            output.getFD().sync();
        }
        Files.move(temporary.toPath(), destination.toPath(), java.nio.file.StandardCopyOption.ATOMIC_MOVE,
                java.nio.file.StandardCopyOption.REPLACE_EXISTING);
    }

    private void updateStatus(String status) {
        uploadStatus = status;
        MainActivity.publishHostUpdate();
    }

    void close() {
        finish();
        if (connectivity != null) connectivity.unregisterNetworkCallback(networkCallback);
        executor.shutdown();
    }
}
