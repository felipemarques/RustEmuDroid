# Desktop releases

1. Choose the next SemVer version for the feature or fix. Increment the minor version for a feature or the patch version for a bug fix. A minor increment resets the patch to zero.
2. Update the RustEmuDroid version in `Cargo.toml` and `Cargo.lock` in the same change. Run `cargo check --locked --features ui` to verify the lockfile.
3. Push or merge the version increase to `main`. **Desktop builds** verifies both archives. A Linux runner extracts the packaged executable, places an Android 15 AOSP runtime beside it, and checks discovery, KVM, `sys.boot_completed=1` and shutdown. The workflow then tags the tested commit as `vX.Y.Z` and publishes a [GitHub Release](https://github.com/felipemarques/RustEmuDroid/releases) with both archives. The `Linux-portable-boot` Actions artifact contains the guest startup log and CLI output.
4. Check that the release includes `RustEmuDroid-vX.Y.Z-windows-x86_64.zip` and `RustEmuDroid-vX.Y.Z-linux-x86_64.tar.gz`.

The hosted Linux job also opens the packaged desktop client in a virtual X11 display with a disposable Android profile. It installs game and app fixtures, sends mouse-wheel input to the desktop window and checks which cards Android shows in each tab. The `Linux-portable-boot` artifact includes the results in `home-wheel.txt`.

Starting in v0.13.0, both packaged clients preview each supported image from the live Google catalog and verify that preview creates no runtime directory. The v0.13.0 Linux boot job repacked the runner's installed Android packages and booted the resulting runtime. Starting in v0.13.1 the release workflows download Google's published archives through the packaged client using the runner's existing SDK license records. Windows initializes the Google Play profile. Linux boots the downloaded AOSP runtime and also exercises installation from local archives. The `runtime-install.txt` file in the Linux portable boot report records this offline installation check.

Pushes that keep the Cargo version, pull requests, and manual workflow runs produce Actions artifacts. If a build or publish step fails, rerun the failed jobs. The publish step can resume an incomplete draft for the same commit.

## Manual tag fallback

If the automatic release was skipped, use the version from the intended release commit to create and push a matching annotated tag. This triggers the same build matrix and publishes both verified archives.

```sh
git tag -a vX.Y.Z <release-commit> -m "RustEmuDroid vX.Y.Z"
git push origin vX.Y.Z
```

Keep published tags attached to their original commits and use a new version for subsequent fixes.
