# Glow video comparison baseline

Run the standalone viewer from the repository root on an idle desktop. GPUI and Glow use the same source and one opaque, aspect-fitted video quad on a black window. Keep the physical window size, monitor and input flags fixed across runs.

```powershell
cargo run --locked --release --manifest-path experiments/glow-baseline/Cargo.toml -- `
  --width 1600 --height 900 --fps 90 --seconds 10 --warmup 2 `
  --window-width 1040 --window-height 585
```

Omit `--raw` for deterministic moving RGBA8 frames. Pass `--raw <file>` or `--raw -` for an equally packed RGBA8 stream with no headers. Use the same source for both viewers. The binary opens no Android connection.

The viewer waits until the Glow paint pass reports the requested physical framebuffer size. It runs the product's native texture uploader and counts each measured sequence once in a callback after its draw command. Standard output contains one JSON report with the OpenGL renderer, distinct sequences, producer replacements, latency from publication to the paint callback, and the effective client size. The timing covers submitted paint work through the callback. Physical presentation requires an independent measurement. Collect process CPU and memory with the same external operator for both viewers.
