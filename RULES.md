# Project rules

- Preserve installed Android profiles across upgrades. Keep the signing identity of the bundled Home APK. When changing Home, update its manifest version and the version checked by the Rust installer together.
- Keep ownership of each VM explicit. Scope ADB operations to its verified serial and profile. Close only the session created by the operation. Wait for the VM to exit before closing its native window.
- Keep desktop and Home text available in Brazilian Portuguese, US English and Spanish from Spain. Brazilian Portuguese is the fallback for fresh installations and older preferences. Keep app names, Android package IDs, log output from external tools and commands intact when translating descriptions.
- Keep disk images, overlays, user apps, accounts, saves and signing keys out of Git. Run foreign VM boot experiments on verified local copies with disposable writable overlays. Treat the original installation and the verified baseline as read-only.
- Use the game’s frame report for game FPS. Identify the actual GPU, Android image, workload and preset when publishing measurements. The desktop counter measures frames presented by the panel.
- Record the backend and boot milestone for compatibility claims. A player opening, a kernel starting and Android reporting boot completion are distinct observations.
