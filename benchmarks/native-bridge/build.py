"""Build the two APKs from a local Android SDK, NDK and JDK."""
import argparse
from hashlib import sha256
import json
import os
from pathlib import Path
import platform
import re
import subprocess
from tempfile import TemporaryDirectory
import zipfile
import first_use

SOURCE = Path(__file__).resolve().parent
NAMESPACE = "org.rustemudroid.qa.abicost"
ACTIVITY = f"{NAMESPACE}.BenchActivity"
LIBRARY = "librustabicost.so"
ABIS = {
    "x86_64": ("x86_64-linux-android26", f"{NAMESPACE}.x86_64", 62),
    "arm64-v8a": ("aarch64-linux-android26", f"{NAMESPACE}.arm64", 183),
}
FLAGS = ["-std=gnu11", "-O2", "-fno-vectorize", "-fno-slp-vectorize", "-fPIC", "-fvisibility=hidden",
         "-shared", "-Wl,-z,max-page-size=16384", "-Wl,--no-undefined", "-Wall", "-Wextra", "-Werror"]
# Derived independently with unsigned 64-bit arithmetic and the same permutation.
GOLDEN = {"warmups": 2, "arithmetic_rounds": 6_000_000, "memory_rounds": 1_048_576,
          "memory_words": 262_144, "arith_digest": "b6fae11725cce32b", "memory_digest": "00fffe953f9f42b0"}


def digest(path):
    return sha256(path.read_bytes()).hexdigest()


def run(*argv):
    return subprocess.check_output(list(map(str, argv)), encoding="utf-8", errors="replace", timeout=120)


def manifest(package):
    return f'''<manifest xmlns:android="http://schemas.android.com/apk/res/android"
    package="{package}" android:versionCode="1" android:versionName="1">
    <uses-sdk android:minSdkVersion="26" android:targetSdkVersion="32" />
    <application android:label="RustEmuDroid ABI benchmark" android:allowBackup="false"
                 android:extractNativeLibs="true"
                 android:theme="@android:style/Theme.Material.Light.NoActionBar">
        <activity android:name="{ACTIVITY}" android:excludeFromRecents="true" android:exported="true" />
    </application>
</manifest>
'''


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--sdk", type=Path, required=True)
    parser.add_argument("--jdk", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--first-use", action="store_true", help="Build the code-traversal workload")
    parser.add_argument("--ndk", default="30.0.14904198")
    parser.add_argument("--build-tools", default="37.0.0")
    parser.add_argument("--platform", default="android-37.0")
    parser.add_argument("--host-tag", help="NDK LLVM prebuilt directory name, if different from the platform default")
    args = parser.parse_args()
    sdk, jdk, out = (path.resolve() for path in (args.sdk, args.jdk, args.out))
    host = args.host_tag or {"Windows": "windows-x86_64", "Linux": "linux-x86_64",
                             "Darwin": "darwin-x86_64"}.get(platform.system())
    if not host:
        parser.error("Specify the installed NDK LLVM prebuilt directory with --host-tag")
    suffix = ".exe" if os.name == "nt" else ""
    tools = sdk / "build-tools" / args.build_tools
    android_jar = sdk / "platforms" / args.platform / "android.jar"
    llvm = sdk / "ndk" / args.ndk / "toolchains/llvm/prebuilt" / host
    clang = llvm / "bin" / f"clang{suffix}"
    readelf = llvm / "bin" / f"llvm-readelf{suffix}"
    java, javac, keytool = (jdk / "bin" / f"{tool}{suffix}" for tool in ("java", "javac", "keytool"))
    aapt, align = (tools / f"{tool}{suffix}" for tool in ("aapt", "zipalign"))
    d8, signer = (tools / "lib" / f"{tool}.jar" for tool in ("d8", "apksigner"))

    out.mkdir(parents=True, exist_ok=True)
    key = out / "fixture-signing.p12"
    password = "fixture-only"
    if not key.exists():
        run(keytool, "-genkeypair", "-storetype", "PKCS12", "-keystore", key, "-storepass", password,
            "-keypass", password, "-alias", "abi-fixture", "-keyalg", "RSA", "-keysize", "2048",
            "-dname", "CN=RustEmuDroid ABI fixture", "-validity", "3650")
    packages = {}
    with TemporaryDirectory(prefix="native-bridge-build-") as directory:
        work = Path(directory)
        classes = work / "classes"
        classes.mkdir()
        run(javac, "--release", "8", "-encoding", "UTF-8", "-classpath", android_jar,
            "-d", classes, SOURCE / "BenchActivity.java")
        run(java, "-cp", d8, "com.android.tools.r8.D8", "--min-api", "26", "--lib", android_jar,
            "--output", work, *sorted(classes.rglob("*.class")))
        dex = work / "classes.dex"
        dex_hash = digest(dex)
        c_source = SOURCE / ("first_use.c" if args.first_use else "bench.c")
        sources = [c_source, SOURCE / "timing.h", SOURCE / "BenchActivity.java"]
        if args.first_use:
            first_use.generate(work)
            sources += [SOURCE / "first_use.py", work / "first_use.inc"]
        source_hashes = {path.name: digest(path) for path in sources}
        for abi, (target, package, machine) in ABIS.items():
            xml, native, unsigned, aligned = (work / name for name in
                                               ("AndroidManifest.xml", LIBRARY, "unsigned.apk", "aligned.apk"))
            apk = out / f"rust-abicost-{abi}.apk"
            xml.write_text(manifest(package), encoding="utf-8")
            run(clang, f"--target={target}", f"--sysroot={llvm / 'sysroot'}", *FLAGS,
                "-I", work, c_source, "-o", native)
            if args.first_use:
                symbols = run(llvm / "bin" / f"llvm-nm{suffix}", "--defined-only", native)
                addresses = re.findall(r"^([0-9a-fA-F]+) [tT] leaf_\d+$", symbols, re.M)
                if len(set(addresses)) != first_use.BANKS * first_use.LEAVES:
                    raise RuntimeError(f"The linker did not preserve every distinct code address for {abi}")
            header = native.read_bytes()[:20]
            if header[:6] != b"\x7fELF\x02\x01" or int.from_bytes(header[18:20], "little") != machine:
                raise RuntimeError(f"Unexpected native image for {abi}")
            if "Java_org_rustemudroid_qa_abicost_BenchActivity_measure" not in run(readelf, "--dyn-syms", "--wide", native):
                raise RuntimeError(f"Missing JNI entry point for {abi}")
            run(aapt, "package", "-f", "-M", xml, "-I", android_jar, "-F", unsigned)
            with zipfile.ZipFile(unsigned, "a") as bundle:
                bundle.write(dex, "classes.dex", compress_type=zipfile.ZIP_DEFLATED)
                bundle.write(native, f"lib/{abi}/{LIBRARY}", compress_type=zipfile.ZIP_STORED)
            run(align, "-P", "16", "-f", "4", unsigned, aligned)
            run(java, "-jar", signer, "sign", "--ks", key, "--ks-key-alias", "abi-fixture",
                "--ks-pass", f"pass:{password}", "--key-pass", f"pass:{password}", "--out", apk, aligned)
            run(java, "-jar", signer, "verify", apk)
            run(align, "-c", "-P", "16", "4", apk)
            metadata = run(aapt, "dump", "badging", apk)
            if f"package: name='{package}'" not in metadata or f"native-code: '{abi}'" not in metadata or "uses-permission" in metadata:
                raise RuntimeError(f"Unexpected APK manifest or ABI for {abi}")
            packages[abi] = {"apk": apk.name, "sha256": digest(apk), "elf_sha256": digest(native),
                             "package": package, "activity": ACTIVITY, "target": target}
    receipt = {"packages": packages, "golden": first_use.golden() if args.first_use else GOLDEN,
               "compiler": run(clang, "--version").splitlines()[0],
               "flags": FLAGS, "ndk": args.ndk, "build_tools": args.build_tools, "platform": args.platform,
               "dex_sha256": dex_hash, "sources": source_hashes}
    path = out / "build-receipt.json"
    path.write_text(json.dumps(receipt, indent=2) + "\n", encoding="utf-8")
    print(path)


if __name__ == "__main__":
    main()
