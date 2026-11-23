package org.rustemudroid.home;

import android.animation.Animator;
import android.animation.AnimatorListenerAdapter;
import android.animation.ObjectAnimator;
import android.animation.PropertyValuesHolder;
import android.animation.ValueAnimator;
import android.content.Context;
import android.database.ContentObserver;
import android.graphics.Typeface;
import android.os.Handler;
import android.os.Looper;
import android.provider.Settings;
import android.text.TextUtils;
import android.view.Gravity;
import android.view.View;
import android.widget.ImageView;
import android.widget.LinearLayout;
import android.widget.TextView;

/** An idle empty state. The shared launcher artwork stays static until the short step. */
final class EmptyLibraryView extends LinearLayout {
  private static final int ICON_LIMIT = 76;
  private static final int ICON_MIN = 32;
  private static final int TEXT_SPACE = 62;
  private final ImageView crab;
  private final TextView message;
  private final ObjectAnimator step;
  private boolean active, scheduled;
  private final Runnable stepOnce = this::stepOnce;
  private final ContentObserver animationScale = new ContentObserver(new Handler(Looper.getMainLooper())) {
    @Override
    public void onChange(boolean selfChange) {
      updateMotion();
    }
  };

  EmptyLibraryView(Context context, int textColor) {
    super(context);
    setOrientation(VERTICAL);
    setGravity(Gravity.CENTER);
    setPadding(dp(12), dp(8), dp(12), dp(8));
    setClipChildren(false);

    crab = new ImageView(context);
    crab.setImageResource(R.drawable.brand);
    crab.setScaleType(ImageView.ScaleType.FIT_CENTER);
    crab.setImportantForAccessibility(View.IMPORTANT_FOR_ACCESSIBILITY_NO);
    addView(crab, dp(ICON_LIMIT), dp(ICON_LIMIT));

    message = new TextView(context);
    message.setGravity(Gravity.CENTER);
    message.setTextColor(textColor);
    message.setTextSize(16);
    message.setTypeface(Typeface.create("sans-serif-medium", Typeface.NORMAL));
    message.setAccessibilityLiveRegion(View.ACCESSIBILITY_LIVE_REGION_POLITE);
    message.setPadding(0, dp(12), 0, 0);
    addView(message, LayoutParams.WRAP_CONTENT, LayoutParams.WRAP_CONTENT);

    float stride = 4 * getResources().getDisplayMetrics().density;
    float lift = stride * 0.45f;
    step = ObjectAnimator.ofPropertyValuesHolder(crab,
        PropertyValuesHolder.ofFloat(View.TRANSLATION_X, 0, -stride, stride, 0),
        PropertyValuesHolder.ofFloat(View.TRANSLATION_Y, 0, -lift, -lift, 0),
        PropertyValuesHolder.ofFloat(View.ROTATION, 0, -4, 4, 0));
    step.setDuration(600);
    step.addListener(new AnimatorListenerAdapter() {
      @Override
      public void onAnimationEnd(Animator animator) {
        if (canStep())
          schedule(2100);
      }
    });
  }

  void present(CharSequence text, boolean animate) {
    if (!TextUtils.equals(message.getText(), text))
      message.setText(text);
    active = animate;
    crab.setVisibility(!TextUtils.isEmpty(text) && iconSize(getHeight()) >= dp(ICON_MIN) ? VISIBLE : GONE);
    updateMotion();
  }

  @Override
  protected void onSizeChanged(int width, int height, int previousWidth, int previousHeight) {
    super.onSizeChanged(width, height, previousWidth, previousHeight);
    // A short landscape window can lose most of its height when the keyboard opens.
    int size = iconSize(height);
    if (crab.getLayoutParams().height != size)
      crab.setLayoutParams(new LayoutParams(size, size));
    crab.setVisibility(!TextUtils.isEmpty(message.getText()) && size >= dp(ICON_MIN) ? VISIBLE : GONE);
    updateMotion();
  }

  @Override
  protected void onAttachedToWindow() {
    super.onAttachedToWindow();
    getContext().getContentResolver().registerContentObserver(
        Settings.Global.getUriFor(Settings.Global.ANIMATOR_DURATION_SCALE), false, animationScale);
    updateMotion();
  }

  @Override
  protected void onDetachedFromWindow() {
    active = false;
    stop();
    getContext().getContentResolver().unregisterContentObserver(animationScale);
    super.onDetachedFromWindow();
  }

  private int iconSize(int availableHeight) {
    return Math.max(0, Math.min(dp(ICON_LIMIT), availableHeight - dp(TEXT_SPACE)));
  }

  private boolean canStep() {
    return active && isAttachedToWindow() && isShown() && crab.isShown()
        && ValueAnimator.areAnimatorsEnabled();
  }

  private void updateMotion() {
    if (canStep()) {
      if (!scheduled && !step.isStarted())
        schedule(350);
    } else {
      stop();
    }
  }

  private void schedule(long delay) {
    scheduled = true;
    postDelayed(stepOnce, delay);
  }

  private void stepOnce() {
    scheduled = false;
    if (canStep())
      step.start();
  }

  private void stop() {
    removeCallbacks(stepOnce);
    scheduled = false;
    if (step.isStarted())
      step.cancel();
    crab.setTranslationX(0);
    crab.setTranslationY(0);
    crab.setRotation(0);
  }

  private int dp(int value) {
    return Math.round(value * getResources().getDisplayMetrics().density);
  }
}
