"""Build the bundled Android Home using the installed SDK and JDK (no Gradle)."""
import argparse
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import zipfile

parser = argparse.ArgumentParser()
parser.add_argument("android_jar", type=Path)
parser.add_argument("build_tools", type=Path)
parser.add_argument("--keystore", type=Path, default=Path.home() / ".android/debug.keystore")
parser.add_argument("--alias", default="androiddebugkey")
args = parser.parse_args()
home = Path(__file__).resolve().parent
suffix = ".exe" if os.name == "nt" else ""
script = ".bat" if os.name == "nt" else ""

def run(*argv):
    subprocess.run(list(map(str,argv)), check=True)

with tempfile.TemporaryDirectory() as tmp:
    work = Path(tmp)
    res = work / "res/drawable-nodpi"
    res.mkdir(parents=True)
    shutil.copyfile(home.parents[1] / "assets/rustemudroid.png", res / "brand.png")
    jar = args.android_jar.resolve()
    bt = args.build_tools.resolve()
    run(bt / ("aapt"+suffix), "package", "-f", "-M",home / "AndroidManifest.xml",
        "-S",res.parent,"-I",jar,"-F",work/"home.apk")
    classes = work / "classes"
    classes.mkdir()
    run("javac","--release","8","-encoding","UTF-8","-classpath",jar,"-d",classes,home/"HomeActivity.java")
    run(bt / ("d8"+script),"--min-api","26","--lib",jar,"--output",work,*classes.rglob("*.class"))
    with zipfile.ZipFile(work/"home.apk","a") as apk:
        apk.write(work/"classes.dex","classes.dex",compress_type=zipfile.ZIP_DEFLATED)
    run(bt / ("zipalign"+suffix),"-f","4",work/"home.apk",work/"aligned.apk")
    # The checked-in prototype APK is signed with the developer's Android debug key.
    # For a custom key, set HOME_KEYSTORE_PASSWORD and pass its alias.
    os.environ.setdefault("HOME_KEYSTORE_PASSWORD","android")
    run(bt / ("apksigner"+script),"sign","--ks",args.keystore,"--ks-key-alias",args.alias,
        "--ks-pass","env:HOME_KEYSTORE_PASSWORD","--out",work/"signed.apk",work/"aligned.apk")
    run(bt / ("apksigner"+script),"verify",work/"signed.apk")
    shutil.copyfile(work/"signed.apk",home/"home.apk")
