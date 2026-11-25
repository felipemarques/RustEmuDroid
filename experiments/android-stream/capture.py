"""Capture RGBA frames from a selected running local AVD for the viewer comparison."""
import argparse
import os
from pathlib import Path
import subprocess
import sys
import threading


def find_session(avd, directory, console):
    roots = [directory] if directory else []
    if directory is None:
        if os.name == "nt":
            roots.append(Path(os.environ["LOCALAPPDATA"]) / "Temp/avd/running")
        else:
            roots.append(avd.parent.parent / "home/avd/running")
            runtime = os.environ.get("XDG_RUNTIME_DIR", f"/run/user/{os.getuid()}")
            roots.append(Path(runtime) / "avd/running")
    matches = []
    for root in dict.fromkeys(roots):
        for ini in root.glob("*.ini"):
            fields = {}
            for line in ini.read_text(encoding="utf-8", errors="replace").splitlines():
                key, _, value = line.partition("=")
                fields[key.strip()] = value.strip()
            if (fields.get("port.serial") == str(console) and fields.get("avd.dir")
                    and Path(fields["avd.dir"]).resolve() == avd
                    and fields.get("grpc.port", "").isdecimal() and fields.get("grpc.token")):
                port = int(fields["grpc.port"])
                if 0 < port <= 65535:
                    matches.append((port, fields["grpc.token"]))
    if len(matches) != 1:
        raise ValueError("Expected one matching local AVD registration. Check --avd, --console and --running-dir.")
    return matches[0]


def capture(args):
    import grpc
    messages, services = grpc.protos_and_services("viewer_frames.proto")
    port, token = find_session(args.avd.resolve(strict=True), args.running_dir, args.console)
    expected = args.width * args.height * 4
    if not (0 < args.width and 0 < args.height and expected <= 63 * 1024 * 1024):
        raise ValueError("Select a positive RGBA frame size below 64 MiB.")
    if min(args.frames, args.fps, args.seconds) <= 0 or (args.timeout is not None and args.timeout <= 0):
        raise ValueError("Frame count, timeout, playback rate and duration must be positive.")
    if args.demo and not args.viewer:
        raise ValueError("--demo requires --viewer pointing to gpui-viewer.")
    timeout = args.timeout if args.timeout is not None else (None if args.demo else max(60, args.seconds + 15))
    options = [("grpc.max_receive_message_length", 64 * 1024 * 1024),
               ("grpc.enable_http_proxy", 0)]
    with grpc.insecure_channel(f"127.0.0.1:{port}", options) as channel:
        stream = services.EmulatorControllerStub(channel).streamScreenshot(
            messages.ImageFormat(format=1, width=args.width, height=args.height),
            timeout=timeout, metadata=(("authorization", "Bearer " + token),))
        child = None
        sink = None
        created = False
        count = 0
        try:
            if args.viewer:
                command = [str(args.viewer.resolve(strict=True)), "--raw", "-",
                    "--width", str(args.width), "--height", str(args.height), "--fps", str(args.fps),
                    "--seconds", str(args.seconds)]
                if args.demo:
                    command.extend(["--demo", "--lang", args.lang])
                child = subprocess.Popen(command, stdin=subprocess.PIPE)
                sink = child.stdin

                def cancel_at_exit():
                    child.wait()
                    stream.cancel()

                threading.Thread(target=cancel_at_exit, daemon=True).start()
            else:
                sink = args.output.open("xb")
                created = True
            try:
                for frame in stream:
                    if not frame.image:
                        continue
                    if ((frame.format.width, frame.format.height) != (args.width, args.height)
                            or frame.format.format != 1 or len(frame.image) != expected):
                        raise ValueError("Android returned a different frame size or format. Select the current aspect ratio.")
                    sink.write(frame.image)
                    count += 1
                    if args.output and count == args.frames:
                        break
            except (BrokenPipeError, grpc.RpcError) as exc:
                if child is None or child.wait(timeout=3) != 0 or not count:
                    raise RuntimeError("The viewer or Android stream ended before capture finished.") from exc
            if args.output and count != args.frames:
                raise RuntimeError(f"Android closed the stream after {count} frames.")
            if child and child.poll() is None:
                sink.close()
                child.wait(timeout=None if args.demo else args.seconds + 10)
            if child and (child.returncode or not count):
                raise RuntimeError(f"The viewer exited with code {child.returncode} after {count} frames.")
            if not child:
                sink.close()
            created = False
            print(f"Sent {count} complete {args.width}x{args.height} Android RGBA frames.", file=sys.stderr)
        finally:
            stream.cancel()
            if sink and not sink.closed:
                try:
                    sink.close()
                except BrokenPipeError:
                    pass
            if child and child.poll() is None:
                child.terminate()
                child.wait()
            if created:
                args.output.unlink(missing_ok=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--avd", type=Path, required=True, help="Existing .avd directory")
    parser.add_argument("--console", type=int, required=True, help="Port shown in emulator-PORT")
    parser.add_argument("--running-dir", type=Path, help="Override the emulator's avd/running directory")
    parser.add_argument("--width", type=int, default=640)
    parser.add_argument("--height", type=int, default=360)
    dest = parser.add_mutually_exclusive_group(required=True)
    dest.add_argument("--output", type=Path, help="Create a new, headerless RGBA recording")
    dest.add_argument("--viewer", type=Path, help="Send frames to a viewer executable over stdin")
    parser.add_argument("--frames", type=int, default=90, help="Frame count for --output")
    parser.add_argument("--fps", type=int, default=90, help="Viewer input rate for --viewer")
    parser.add_argument("--seconds", type=int, default=10, help="Measured viewer duration")
    parser.add_argument("--demo", action="store_true", help="Open the interactive GPUI viewer until closed")
    parser.add_argument("--lang", choices=["pt-BR", "en-US", "es-ES"], default="pt-BR",
                        help="Language of the interactive GPUI controls")
    parser.add_argument("--timeout", type=int, help="Overall Android stream timeout in seconds")
    try:
        capture(parser.parse_args())
    except (OSError, ValueError, RuntimeError, subprocess.SubprocessError) as error:
        parser.exit(1, f"{error}\n")
