package org.rustemudroid.home;

import android.app.Activity;
import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;
import android.content.SharedPreferences;

public final class HomeLocaleReceiver extends BroadcastReceiver {
  static final String PREFERENCES = "home";
  static final String EXTRA = "locale";
  static final String DEFAULT = "pt-br";
  private static final String ACTION = "org.rustemudroid.home.SET_LOCALE";

  @Override
  public void onReceive(Context context, Intent intent) {
    if (!ACTION.equals(intent.getAction()))
      return;
    String locale = intent.getStringExtra(EXTRA);
    if (!DEFAULT.equals(locale) && !"en-us".equals(locale) && !"es-es".equals(locale))
      return;
    SharedPreferences preferences = context.getSharedPreferences(PREFERENCES, Context.MODE_PRIVATE);
    boolean saved = locale.equals(preferences.getString(EXTRA, null))
        || preferences.edit().putString(EXTRA, locale).commit();
    if (saved)
      setResultCode(Activity.RESULT_OK);
  }
}
