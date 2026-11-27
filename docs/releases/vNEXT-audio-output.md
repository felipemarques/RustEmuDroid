# RustEmuDroid vNEXT (draft): audio output

**Settings → Sound → Audio output** chooses where Android's sound plays. The first entry, **System default**, is selected for new and existing preferences and shows the device the system uses now. With it the client changes nothing: the engine starts with the same arguments and the same environment as before the setting existed. On Windows the engine's audio back end opens the Windows wave mapper and on Linux it opens a PulseAudio stream without a sink, so Android follows the system default output, also when you change it while Android runs. A `QEMU_PA_SINK` that you exported yourself before starting RustEmuDroid still reaches the engine. The other entries are the computer's active output devices by their friendly names. A choice is saved in `launcher.json` (`hardware.audio_output` and its display name) and applies after saving and restarting Android. An older `launcher.json` without these keys loads as **System default**, and saving with **System default** writes no new key.

If the saved device is not connected when Android starts, Android uses the system default. A notice at the bottom right of the window says which device was missing; it stays for 20 seconds counted from the moment Android is on screen, so it is not lost while Android starts. The same line is kept under **Settings → Launcher output**.

## How a chosen device is reached

### Windows

The stock engine has no device option. When a device other than the default is chosen, the engine starts with `-no-audio` and the RustEmuDroid window plays the engine's `streamAudio` gRPC stream (48 kHz stereo, 16-bit) through that device with WASAPI in shared mode. The host microphone is unaffected in practice: the engine already sends silence to Android's microphone unless host audio input is allowed, and `-no-audio` also turns that input off.

- **Buffering.** The player waits for 40 ms of audio before it starts or resumes, repeats or skips a single frame per write when its buffer leaves 30–70 ms, and trims back to 40 ms above 160 ms. In a live session on the first version of this feature the engine sent about 100 packets of 10 ms per second, the packets were 0.2 ms old on arrival, the client held 20–52 ms of audio (queue plus device buffer) before the device's own output latency, and the engine's clock ran about 1 ms per second slower than the device's. The end-to-end delay from Android to the speaker has not been measured.
- **The chosen device becomes unavailable** (unplugged, disabled, taken in exclusive mode by another program). The player tries it once more at once, which is all a format change needs. If that fails, the sound plays on the system default, and follows the system default when you change it: the default endpoint is looked up again every second. The chosen device is tried again every 3 seconds and the sound goes back to it when it works. You are told once each way, and only after the new device is really playing.
- **Nothing can play.** If no output can be opened (also when Windows' device enumerator cannot be created) for 3 seconds, or the engine's audio stream keeps failing for 5 seconds or was never announced, a notice says that Android's sound is not playing, why, and that selecting **System default** and restarting Android returns to the engine's own sound. The player keeps trying, and says so when the sound is back. The notice is not raised while Android shuts down.
- **Where you see it.** These notices use the same pop-up as the capture notices, at the bottom right of the window, over the Android screen, also in fullscreen. **Settings → Sound** shows one line with the state of the running session: playing on the chosen device, playing on the system default because the device is unavailable (with the reason), or not playing (with the reason). Every change is also added to **Launcher output**.
- **Silence.** The device stream starts with the first audible sample and stops 5 seconds after the last one, once the device has played what it was given, so a silent Android does not keep the computer awake or a wireless headset active. Silence shorter than that is played as it is. When sound comes back the player starts from its first audible sample; the 40 ms of buffering above apply as on every start. Per the engine source, `streamAudio` only forwards what Android's output device plays (`EmulatorService.cpp`, `audio/audio.c` `audio_run_out`): it sends nothing while Android's audio output is idle, and packets of zeros while an app keeps a silent track open, so the player decides on the samples, not on the arrival of packets.

### Linux

The client sets `QEMU_PA_SINK` to the chosen PulseAudio sink (PipeWire through pipewire-pulse works the same way) and lists the sinks with `pactl`. `pactl` is only started for a chosen device, when Settings opens and for `audio-outputs`; with **System default** saved, starting Android never runs it. Its output is read while it runs and it is ended after 5 seconds.

While the embedded panel starts the engine with `-no-window`, the headless engine binary has no sound at all (its PulseAudio library is a stub). The selector says so for every entry instead of promising a device, and the launcher output repeats it for a chosen device. Both read the way the panel starts the engine, so the text changes by itself when the panel runs the engine with a hidden window.

## Command line

`rustemudroid audio-outputs` lists the devices with their IDs and marks the system default. `boot --audio-output <ID or name>` chooses one on Linux. An explicit `--audio-output` that cannot be honoured ends with a message and exit code 2, before anything is started:

- no device has that ID or name (the message lists the available devices), or the devices cannot be listed;
- together with `--headless`, where the engine runs without sound;
- on Windows, where the engine only plays on the system default and only the RustEmuDroid window plays on another device.

## Tested

Unit tests: device matching; the boot plan (the system default changes neither arguments nor environment, `-no-audio`, messages, fallback); the checks of an explicit `--audio-output` and the exit code; `pactl` parsing, output larger than a pipe buffer, a hanging and a failing program; an older `launcher.json`, the preference round trip, labels and the selector text with and without engine sound; the gRPC request encoding; the playback queue, priming, the repeated and the skipped frame, the trim; starting and stopping the device stream around silence, with zeros and with no packets; falling back, following the default, returning, and what is announced when; the notice and status texts in the three languages; the notice on screen while Android starts and runs.

On the Windows test PC, with the first version of this feature: the device list matched the system's five active outputs, and an opt-in test played a 1 kHz tone through the player on VB-Audio "CABLE Input" (not the default device) and recorded it back from "CABLE Output" at the expected level and frequency. Live sessions (stock Emulator 37.3.10, official Android 17 Google Play image, a disposable profile, Android ringing for an incoming call from its own emulator console):

- **System default:** the engine (`qemu-system-x86_64.exe`) owned the only RustEmuDroid audio session, on the system default output, with a session peak of 0.16; nothing reached the virtual cable.
- **VB-Audio "CABLE Input" chosen:** the engine ran with `-no-audio` and owned no audio session on any output; the client owned the session on CABLE Input (peak 0.16) and the ringtone was recorded from "CABLE Output" for 6 seconds (peak 5238 of 32767).
- **A saved device that does not exist:** the launcher output said "The saved audio output “Missing Test Speaker” is not connected. Android uses the system default output.", the engine ran without `-no-audio` and the client played nothing.

Not tested with a running Android or a real device since the player changed: starting and stopping the device stream around silence, unplugging and reconnecting the chosen device, changing the system default while the default plays, a device held in exclusive mode, and the notices on screen. Not tested: any Linux desktop.
