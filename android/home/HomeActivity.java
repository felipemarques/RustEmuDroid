package org.rustemudroid.home;

import android.app.Activity;
import android.app.AlertDialog;
import android.content.*;
import android.content.pm.*;
import android.graphics.Typeface;
import android.graphics.drawable.*;
import android.os.Bundle;
import android.text.*;
import android.view.*;
import android.widget.*;
import java.util.*;
import java.util.concurrent.*;

public final class HomeActivity extends Activity {
  private static final int BG = 0xff111720, PANEL = 0xff202b38, TEXT = 0xffedf2f6,
                           MUTED = 0xffadbdca, ACCENT = 0xffffa274;
  private static final int[] TINTS = {0xff3c505b, 0xff43516c, 0xff5b4240, 0xff3f5148};
  private final ExecutorService worker = Executors.newSingleThreadExecutor();
  private final List<Entry> entries = new ArrayList<>(), shown = new ArrayList<>();
  private boolean games = true, visible;
  private int generation;
  private Future<?> pending;
  private EditText search;
  private TextView gamesTab, appsTab, empty;
  private GridView grid;
  private Cards cards;
  private final BroadcastReceiver packages = new BroadcastReceiver() {
    @Override
    public void onReceive(Context context, Intent intent) {
      reload();
    }
  };

  private static final class Entry {
    final ComponentName component;
    final String name;
    final boolean game, banner;
    final Drawable art;
    Entry(ComponentName component, String name, boolean game, Drawable art, boolean banner) {
      this.component = component;
      this.name = name;
      this.game = game;
      this.art = art;
      this.banner = banner;
    }
  }

  @Override
  public void onCreate(Bundle state) {
    super.onCreate(state);
    getWindow().setStatusBarColor(BG);
    getWindow().setNavigationBarColor(BG);
    getWindow().addFlags(WindowManager.LayoutParams.FLAG_FULLSCREEN);
    LinearLayout root = new LinearLayout(this);
    root.setOrientation(LinearLayout.VERTICAL);
    root.setBackgroundColor(BG);
    root.setFocusableInTouchMode(true);
    root.setPadding(dp(20), dp(12), dp(20), dp(8));
    LinearLayout bar = new LinearLayout(this);
    bar.setGravity(Gravity.CENTER_VERTICAL);
    root.addView(bar, -1, dp(44));
    gamesTab = tab(bar, "Jogos", true);
    appsTab = tab(bar, "Aplicativos", false);
    Space gap = new Space(this);
    bar.addView(gap, new LinearLayout.LayoutParams(0, 1, 1f));
    search = new EditText(this);
    search.setTextSize(14);
    search.setSingleLine();
    search.setTextColor(TEXT);
    search.setHintTextColor(MUTED);
    search.setPadding(dp(12), 0, dp(12), 0);
    search.setBackground(shape(PANEL, 8));
    bar.addView(
        search, dp(getResources().getConfiguration().screenWidthDp < 470 ? 116 : 200), dp(36));
    search.addTextChangedListener(new TextWatcher() {
      public void beforeTextChanged(CharSequence s, int start, int count, int after) {}
      public void onTextChanged(CharSequence s, int start, int before, int count) {
        filter();
      }
      public void afterTextChanged(Editable e) {}
    });
    FrameLayout body = new FrameLayout(this);
    LinearLayout.LayoutParams bodyParams = new LinearLayout.LayoutParams(-1, 0, 1);
    bodyParams.topMargin = dp(12);
    root.addView(body, bodyParams);
    grid = new GridView(this);
    grid.setNumColumns(GridView.AUTO_FIT);
    grid.setStretchMode(GridView.STRETCH_COLUMN_WIDTH);
    grid.setColumnWidth(dp(186));
    grid.setHorizontalSpacing(dp(10));
    grid.setVerticalSpacing(dp(10));
    grid.setClipToPadding(false);
    grid.setPadding(dp(2), dp(2), dp(2), dp(8));
    cards = new Cards();
    grid.setAdapter(cards);
    body.addView(grid);
    empty = text("", 15, false);
    empty.setTextColor(MUTED);
    empty.setGravity(Gravity.CENTER);
    body.addView(empty);
    grid.setEmptyView(empty);
    grid.setOnItemClickListener((parent, v, index, id) -> launch(shown.get(index)));
    grid.setOnItemLongClickListener((parent, v, index, id) -> {
      Entry item = shown.get(index);
      new AlertDialog.Builder(this)
          .setTitle(item.name)
          .setItems(new String[] {games ? "Mover para Aplicativos" : "Mover para Jogos"},
              (dialog, which) -> {
                getSharedPreferences("categories", 0)
                    .edit()
                    .putBoolean(item.component.flattenToString(), !games)
                    .apply();
                filter();
              })
          .show();
      return true;
    });
    setContentView(root);
    select(state == null ? !getIntent().getBooleanExtra("apps", false)
                         : state.getBoolean("games", true));
    if (state != null)
      search.setText(state.getString("search", ""));
  }

  @Override
  protected void onNewIntent(Intent intent) {
    super.onNewIntent(intent);
    setIntent(intent);
    select(!intent.getBooleanExtra("apps", false));
  }
  @Override
  protected void onSaveInstanceState(Bundle out) {
    super.onSaveInstanceState(out);
    out.putBoolean("games", games);
    out.putString("search", search.getText().toString());
  }
  @Override
  protected void onStart() {
    super.onStart();
    visible = true;
    IntentFilter filter = new IntentFilter();
    filter.addAction(Intent.ACTION_PACKAGE_ADDED);
    filter.addAction(Intent.ACTION_PACKAGE_REMOVED);
    filter.addAction(Intent.ACTION_PACKAGE_CHANGED);
    filter.addDataScheme("package");
    registerReceiver(packages, filter);
    reload();
  }
  @Override
  protected void onStop() {
    visible = false;
    generation++;
    if (pending != null)
      pending.cancel(true);
    unregisterReceiver(packages);
    super.onStop();
  }
  @Override
  protected void onDestroy() {
    worker.shutdownNow();
    super.onDestroy();
  }

  private void reload() {
    if (!visible)
      return;
    final int request = ++generation;
    if (pending != null)
      pending.cancel(true);
    pending = worker.submit(() -> {
      List<Entry> items = new ArrayList<>();
      PackageManager pm = getPackageManager();
      Intent query = new Intent(Intent.ACTION_MAIN).addCategory(Intent.CATEGORY_LAUNCHER);
      Set<ComponentName> seen = new HashSet<>();
      for (ResolveInfo r : pm.queryIntentActivities(query, 0)) {
        if (Thread.currentThread().isInterrupted())
          return;
        ActivityInfo activity = r.activityInfo;
        ComponentName component = new ComponentName(activity.packageName, activity.name);
        if (activity.packageName.equals(getPackageName()) || !seen.add(component))
          continue;
        try {
          ApplicationInfo app = activity.applicationInfo;
          boolean game = app.category == ApplicationInfo.CATEGORY_GAME
              || (app.flags & ApplicationInfo.FLAG_IS_GAME) != 0;
          Drawable art = activity.loadBanner(pm);
          boolean banner = art != null;
          if (!banner)
            art = r.loadIcon(pm);
          items.add(new Entry(component, r.loadLabel(pm).toString(), game, art, banner));
        } catch (RuntimeException ignored) { /* Package replaced during enumeration. */
        }
      }
      items.sort((a, b) -> a.name.compareToIgnoreCase(b.name));
      runOnUiThread(() -> {
        if (!visible || generation != request)
          return;
        entries.clear();
        entries.addAll(items);
        filter();
      });
    });
  }

  private void launch(Entry item) {
    Intent intent = Intent.makeMainActivity(item.component);
    try {
      startActivity(intent);
    } catch (ActivityNotFoundException | SecurityException ex) {
      Toast.makeText(this, "O Android não conseguiu abrir " + item.name, Toast.LENGTH_SHORT).show();
      reload();
    }
  }

  private TextView tab(LinearLayout row, String label, boolean isGames) {
    TextView v = text(label, 16, true);
    v.setGravity(Gravity.CENTER);
    v.setPadding(dp(12), 0, dp(12), 0);
    v.setFocusable(true);
    v.setOnClickListener(view -> select(isGames));
    LinearLayout.LayoutParams lp = new LinearLayout.LayoutParams(-2, dp(36));
    lp.rightMargin = dp(4);
    row.addView(v, lp);
    return v;
  }
  private void select(boolean value) {
    games = value;
    for (TextView v : new TextView[] {gamesTab, appsTab}) {
      boolean selected = (v == gamesTab) == games;
      v.setBackground(shape(selected ? ACCENT : PANEL, 8));
      v.setTextColor(selected ? BG : TEXT);
      v.setSelected(selected);
    }
    search.setHint(games ? "Buscar jogo" : "Buscar aplicativo");
    search.setText("");
    search.clearFocus();
    filter();
    grid.setSelection(0);
  }
  private void filter() {
    if (cards == null)
      return;
    String term = search.getText().toString().trim().toLowerCase(Locale.ROOT);
    SharedPreferences prefs = getSharedPreferences("categories", 0);
    shown.clear();
    for (Entry e : entries) {
      if (prefs.getBoolean(e.component.flattenToString(), e.game) == games
          && (e.name.toLowerCase(Locale.ROOT).contains(term)
              || e.component.getPackageName().toLowerCase(Locale.ROOT).contains(term)))
        shown.add(e);
    }
    empty.setText(term.isEmpty() ? (games ? "Nenhum jogo instalado" : "Nenhum aplicativo instalado")
                                 : "Nenhum resultado");
    cards.notifyDataSetChanged();
  }

  private final class Cards extends BaseAdapter {
    public int getCount() {
      return shown.size();
    }
    public Object getItem(int i) {
      return shown.get(i);
    }
    public long getItemId(int i) {
      return i;
    }
    public View getView(int i, View convert, ViewGroup parent) {
      Entry item = shown.get(i);
      LinearLayout card = (LinearLayout) convert;
      if (card == null) {
        card = new LinearLayout(HomeActivity.this);
        card.setOrientation(LinearLayout.VERTICAL);
        card.setBackground(shape(PANEL, 10));
        card.setClipToOutline(true);
        ImageView image = new ImageView(HomeActivity.this);
        card.addView(image, new LinearLayout.LayoutParams(-1, 0, 1));
        TextView name = text("", 15, true);
        name.setSingleLine(true);
        name.setEllipsize(TextUtils.TruncateAt.END);
        name.setGravity(Gravity.CENTER_VERTICAL);
        name.setPadding(dp(10), 0, dp(8), 0);
        card.addView(name, -1, dp(36));
      }
      card.setLayoutParams(new AbsListView.LayoutParams(-1, dp(games ? 134 : 97)));
      ImageView image = (ImageView) card.getChildAt(0);
      int inset = dp(item.banner && games ? 0 : 8);
      image.setPadding(inset, inset, inset, inset);
      image.setScaleType(
          item.banner && games ? ImageView.ScaleType.CENTER_CROP : ImageView.ScaleType.FIT_CENTER);
      image.setBackground(new GradientDrawable(GradientDrawable.Orientation.TL_BR,
          new int[] {TINTS[Math.floorMod(item.component.hashCode(), TINTS.length)], PANEL}));
      image.setImageDrawable(item.art);
      ((TextView) card.getChildAt(1)).setText(item.name);
      card.setContentDescription(item.name);
      return card;
    }
  }
  private TextView text(String value, int size, boolean bold) {
    TextView v = new TextView(this);
    v.setText(value);
    v.setTextSize(size);
    v.setTextColor(TEXT);
    if (bold)
      v.setTypeface(Typeface.create("sans-serif-medium", Typeface.NORMAL));
    return v;
  }
  private GradientDrawable shape(int color, int radius) {
    GradientDrawable d = new GradientDrawable();
    d.setColor(color);
    d.setCornerRadius(dp(radius));
    return d;
  }
  private int dp(int v) {
    return Math.round(v * getResources().getDisplayMetrics().density);
  }
}
