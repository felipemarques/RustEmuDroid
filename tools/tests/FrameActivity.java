package org.rustemudroid.qa.games;

import android.app.Activity;
import android.graphics.Canvas;
import android.os.Bundle;
import android.view.SurfaceHolder;
import android.view.SurfaceView;

/** An animated game surface for the Linux title-bar FPS integration test. */
public final class FrameActivity extends Activity implements SurfaceHolder.Callback, Runnable {
    private SurfaceView view;
    private int frame;

    @Override public void onCreate(Bundle saved) {
        super.onCreate(saved);
        view = new SurfaceView(this);
        view.getHolder().addCallback(this);
        setContentView(view);
    }

    @Override public void surfaceCreated(SurfaceHolder holder) { view.post(this); }
    @Override public void surfaceDestroyed(SurfaceHolder holder) { view.removeCallbacks(this); }
    @Override public void surfaceChanged(SurfaceHolder holder, int format, int w, int h) {}

    @Override public void run() {
        SurfaceHolder holder = view.getHolder();
        if (!holder.getSurface().isValid()) return;
        Canvas canvas = holder.lockCanvas();
        if (canvas != null) {
            try {
                canvas.drawColor((frame++ % 2 == 0) ? 0xff284060 : 0xff304868);
            } finally {
                holder.unlockCanvasAndPost(canvas);
            }
        }
        view.postDelayed(this, 80);
    }
}
