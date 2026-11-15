package dev.gpui.lab;

import android.content.ActivityNotFoundException;
import android.content.Intent;
import android.util.Log;
import android.widget.Toast;
import dev.gpui.mobile.GpuiInputActivity;

/** Android owns the native surface and input bridge; GPUI draws the app. */
public final class MainActivity extends GpuiInputActivity {
    /** Called by Rust. Successful return means the UI request was scheduled. */
    public void shareReport(String report) {
        if (report == null || report.isEmpty()) {
            throw new IllegalArgumentException("The test report is empty.");
        }
        if (isFinishing() || isDestroyed()) {
            throw new IllegalStateException("The app activity is closing.");
        }
        runOnUiThread(() -> {
            try {
                if (isFinishing() || isDestroyed()) {
                    throw new IllegalStateException("The app activity is closing.");
                }
                Intent send = new Intent(Intent.ACTION_SEND);
                send.setType("text/plain");
                send.putExtra(Intent.EXTRA_SUBJECT, "GPUI Lab 测试结果");
                send.putExtra(Intent.EXTRA_TEXT, report);
                startActivity(Intent.createChooser(send, "分享测试结果"));
            } catch (ActivityNotFoundException | SecurityException | IllegalStateException error) {
                Log.e("GpuiLab", "Could not open the report share chooser", error);
                Toast.makeText(this, "无法打开分享，请重试。" + error.getMessage(), Toast.LENGTH_LONG).show();
            }
        });
    }
}
