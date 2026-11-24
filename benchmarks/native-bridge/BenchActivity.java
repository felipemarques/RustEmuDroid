package org.rustemudroid.qa.abicost;

import android.app.Activity;
import android.os.Bundle;
import android.os.Handler;
import android.os.Looper;
import android.os.Process;
import android.util.Log;
import android.widget.TextView;
import org.json.JSONObject;

public final class BenchActivity extends Activity {
    private static final String TAG = "RustAbiCost";
    private static native String measure(int samples);

    @Override protected void onCreate(Bundle state) {
        super.onCreate(state);
        String nonce = getIntent().getStringExtra("nonce");
        int samples = getIntent().getIntExtra("samples", 7);
        if (nonce == null || !nonce.matches("[0-9a-f]{32}") || samples < 1 || samples > 9) {
            finish();
            return;
        }
        TextView view = new TextView(this);
        view.setText("RustEmuDroid ABI benchmark");
        setContentView(view);
        new Handler(Looper.getMainLooper()).postDelayed(() -> {
            new Thread(() -> execute(nonce, samples), "RustAbiCost").start();
        }, 900);
    }

    private void execute(String nonce, int samples) {
        try {
            System.loadLibrary("rustabicost");
            JSONObject result = new JSONObject(measure(samples));
            result.put("package", getPackageName()).put("pid", Process.myPid())
                  .put("tid", Process.myTid()).put("process_64bit", Process.is64Bit());
            Log.i(TAG, nonce + " " + result);
        } catch (Throwable error) {
            Log.e(TAG, nonce + " ERROR " + error);
        } finally {
            runOnUiThread(this::finish);
        }
    }
}
