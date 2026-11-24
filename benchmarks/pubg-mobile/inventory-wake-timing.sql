-- Run with Perfetto Trace Processor on a PUBG Inventory trace.
-- Requires sched_switch, sched_waking and the Android gfx/view slices.
WITH game_threads AS (
    SELECT t.utid, t.name
    FROM thread t JOIN process p USING (upid)
    WHERE p.name = 'com.tencent.ig'
), producer_events AS (
    SELECT DISTINCT s.ts
    FROM slice s JOIN thread_track tt ON tt.id = s.track_id
    JOIN game_threads t USING (utid)
    WHERE t.name = 'RHIThread' AND s.dur >= 0
      AND s.name LIKE 'onFrameAvailable%SurfaceView[com.tencent.ig/com.epicgames.ue4.GameActivity]%'
), producer_intervals AS (
    SELECT lag(ts) OVER (ORDER BY ts) AS start_ns, ts AS end_ns
    FROM producer_events
), gap AS (
    SELECT start_ns, end_ns FROM producer_intervals
    WHERE start_ns IS NOT NULL
    ORDER BY end_ns - start_ns DESC, start_ns LIMIT 1
), ranked_threads AS (
    SELECT t.utid, t.name, row_number() OVER (
        PARTITION BY t.name ORDER BY SUM(s.dur) DESC, t.utid
    ) AS rank
    FROM thread_state s JOIN game_threads t USING (utid)
    WHERE s.state = 'Running' AND s.dur > 0
      AND t.name IN ('UEGameThread', 'RHIThread', 'RenderThread 1')
    GROUP BY t.utid, t.name
), main_threads AS (
    SELECT utid, name FROM ranked_threads WHERE rank = 1
), waits AS (
    SELECT t.name AS thread, w.name AS waker,
           MAX(g.start_ns, s.ts) AS start_ns, MIN(g.end_ns, s.ts + s.dur) AS end_ns
    FROM gap g JOIN thread_state s ON s.ts < g.end_ns AND s.ts + s.dur > g.start_ns
    JOIN main_threads t USING (utid)
    JOIN thread_state next ON next.utid = s.utid AND next.ts = s.ts + s.dur
    JOIN game_threads w ON w.utid = next.waker_utid
    WHERE s.state = 'S' AND s.dur > 0 AND next.state = 'R' AND next.irq_context = 0
), render_waits AS (
    SELECT * FROM waits
    WHERE thread = 'RenderThread 1' AND end_ns - start_ns >= 50000000
), cpu AS (
    SELECT s.ts, s.ts + s.dur AS end_ns, t.name
    FROM thread_state s JOIN game_threads t USING (utid)
    WHERE s.dur > 0 AND s.state = 'Running' AND (
        t.name = 'FAsyncLoading' OR s.utid IN (SELECT utid FROM main_threads)
    )
), render_cpu AS (
    SELECT w.start_ns, w.end_ns, w.waker,
           SUM(CASE WHEN c.name = 'UEGameThread' THEN
               MIN(c.end_ns, w.end_ns) - MAX(c.ts, w.start_ns) END) AS game_ns,
           SUM(CASE WHEN c.name = 'FAsyncLoading' THEN
               MIN(c.end_ns, w.end_ns) - MAX(c.ts, w.start_ns) END) AS loader_ns,
           SUM(CASE WHEN c.name = 'RHIThread' THEN
               MIN(c.end_ns, w.end_ns) - MAX(c.ts, w.start_ns) END) AS rhi_ns
    FROM render_waits w LEFT JOIN cpu c ON c.ts < w.end_ns AND c.end_ns > w.start_ns
    GROUP BY w.start_ns, w.end_ns, w.waker
), api_windows AS (
    SELECT s.name, MAX(g.start_ns, s.ts) AS start_ns, MIN(g.end_ns, s.ts + s.dur) AS end_ns
    FROM gap g JOIN slice s ON s.ts < g.end_ns AND s.ts + s.dur > g.start_ns
    JOIN thread_track tt ON tt.id = s.track_id JOIN game_threads t USING (utid)
    WHERE s.dur > 0 AND t.name = 'RHIThread'
      AND s.name IN ('eglSwapBuffers', 'QueuePresentKHR', 'queueBuffer', 'dequeueBuffer', 'AcquireNextImageKHR')
), results AS (
    SELECT 'Producer gap' AS metric, '' AS detail, 0 AS start_ns, end_ns - start_ns AS end_ns,
           end_ns - start_ns AS duration_ns, NULL AS game_ns, NULL AS loader_ns, NULL AS rhi_ns
    FROM gap
    UNION ALL
    SELECT 'RHI sleep, total', waker, NULL, NULL, SUM(end_ns - start_ns), NULL, NULL, NULL
    FROM waits WHERE thread = 'RHIThread' GROUP BY waker
    UNION ALL
    SELECT 'Render sleep', waker, w.start_ns - g.start_ns, w.end_ns - g.start_ns,
           w.end_ns - w.start_ns, game_ns, loader_ns, rhi_ns
    FROM render_cpu w CROSS JOIN gap g
    UNION ALL
    SELECT 'API span', name, w.start_ns - g.start_ns, w.end_ns - g.start_ns,
           w.end_ns - w.start_ns, NULL, NULL, NULL
    FROM api_windows w CROSS JOIN gap g
)
SELECT metric, detail, ROUND(start_ns / 1e6, 3) AS start_ms, ROUND(end_ns / 1e6, 3) AS end_ms,
       ROUND(duration_ns / 1e6, 3) AS duration_ms, ROUND(game_ns / 1e6, 3) AS game_cpu_ms,
       ROUND(loader_ns / 1e6, 3) AS loader_cpu_ms, ROUND(rhi_ns / 1e6, 3) AS rhi_cpu_ms
FROM results ORDER BY metric, start_ns, detail;
