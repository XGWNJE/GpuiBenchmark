package dev.gpui.lab;

import android.content.ActivityNotFoundException;
import android.content.Intent;
import android.graphics.Insets;
import android.os.Build;
import android.os.Bundle;
import android.util.Log;
import android.view.View;
import android.view.WindowInsets;
import android.view.WindowInsetsController;
import android.widget.Toast;
import dev.gpui.mobile.GpuiInputActivity;

/** Android owns the native surface and input bridge; GPUI draws the app. */
public final class MainActivity extends GpuiInputActivity {
    private static native void publishInsets(int left, int top, int right, int bottom);

    @Override protected void onCreate(Bundle state) {
        super.onCreate(state);
        View decor = getWindow().getDecorView();
        decor.setOnApplyWindowInsetsListener((view, insets) -> {
            int left;
            int top;
            int right;
            int bottom;
            if (Build.VERSION.SDK_INT >= 30) {
                Insets safe = insets.getInsets(WindowInsets.Type.systemBars()
                        | WindowInsets.Type.displayCutout() | WindowInsets.Type.ime());
                left = safe.left;
                top = safe.top;
                right = safe.right;
                bottom = safe.bottom;
            } else {
                left = insets.getSystemWindowInsetLeft();
                top = insets.getSystemWindowInsetTop();
                right = insets.getSystemWindowInsetRight();
                bottom = insets.getSystemWindowInsetBottom();
            }
            publishInsets(left, top, right, bottom);
            return insets;
        });
        decor.requestApplyInsets();
    }

    public void setLightSystemBars(boolean light) {
        runOnUiThread(() -> {
            if (Build.VERSION.SDK_INT >= 30) {
                WindowInsetsController controller = getWindow().getInsetsController();
                if (controller != null) {
                    int mask = WindowInsetsController.APPEARANCE_LIGHT_STATUS_BARS
                            | WindowInsetsController.APPEARANCE_LIGHT_NAVIGATION_BARS;
                    controller.setSystemBarsAppearance(light ? mask : 0, mask);
                }
            } else {
                View decor = getWindow().getDecorView();
                int mask = View.SYSTEM_UI_FLAG_LIGHT_STATUS_BAR | View.SYSTEM_UI_FLAG_LIGHT_NAVIGATION_BAR;
                int flags = decor.getSystemUiVisibility();
                decor.setSystemUiVisibility(light ? flags | mask : flags & ~mask);
            }
        });
    }

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
