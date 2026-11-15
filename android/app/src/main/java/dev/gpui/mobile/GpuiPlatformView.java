package dev.gpui.mobile;

/** Lifecycle entry points expected by the mobile backend. GPUI owns all app views. */
public final class GpuiPlatformView {
    private GpuiPlatformView() {}

    public static void pauseAll() {}

    public static void resumeAll() {}

    public static void disposeAll() {}
}
