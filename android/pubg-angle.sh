# Run inside the verified Android 17 Google Play guest. ANGLE's package and
# driver settings are parallel comma-separated lists managed by Android.
pkgs_key=angle_gl_driver_selection_pkgs
drivers_key=angle_gl_driver_selection_values
pkgs=$(settings get global "$pkgs_key") || exit 1
drivers=$(settings get global "$drivers_key") || exit 1
case ",$pkgs," in *,com.tencent.ig,*) exit 0 ;; esac

if { [ "$pkgs" = null ] && [ "$drivers" = null ]; } || { [ -z "$pkgs" ] && [ -z "$drivers" ]; }; then
    new_pkgs=com.tencent.ig
    new_drivers=angle
elif [ -n "$pkgs" ] && [ "$pkgs" != null ] && [ -n "$drivers" ] && [ "$drivers" != null ]; then
    # Leave malformed or partially configured tables to their owner.
    case "$pkgs,$drivers" in ,*|*,|*,,*) exit 0 ;; esac
    [ "$(printf %s "$pkgs" | tr -cd ',')" = "$(printf %s "$drivers" | tr -cd ',')" ] || exit 0
    new_pkgs="$pkgs,com.tencent.ig"
    new_drivers="$drivers,angle"
else
    exit 0
fi
[ -r /system/lib64/libEGL_angle.so ] || exit 0
[ "$(pm has-feature android.hardware.vulkan.version 4198400)" = true ] || exit 0

# Add the driver first so a package never picks up another app's selection.
[ "$(settings get global "$pkgs_key")" = "$pkgs" ] &&
    [ "$(settings get global "$drivers_key")" = "$drivers" ] || exit 0
settings put global "$drivers_key" "$new_drivers" || exit 1
[ "$(settings get global "$drivers_key")" = "$new_drivers" ] || exit 1
if [ "$(settings get global "$pkgs_key")" = "$pkgs" ] &&
   settings put global "$pkgs_key" "$new_pkgs" &&
   [ "$(settings get global "$pkgs_key")" = "$new_pkgs" ]; then
    echo applied
elif [ "$(settings get global "$drivers_key")" = "$new_drivers" ]; then
    if [ "$drivers" = null ]; then
        settings delete global "$drivers_key" >/dev/null
    else
        settings put global "$drivers_key" "$drivers"
    fi
    exit 1
fi
