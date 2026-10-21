# PipeWire DJ

PipeWire DJ mixes sound clips, your real microphone and other apps' audio into one PipeWire virtual microphone called **PipeWire DJ**. Any program that records from a microphone can use it: games, Discord, OBS, browsers. You can keep talking while a clip plays.

It is a from-scratch successor to [SLAM](https://github.com/SilentSys/SLAM), which stopped working when Counter-Strike 2 removed `voice_inputfromfile`. Linux comes first; Windows support is planned.

## Features

- Virtual microphone that mixes mic, track and selected app audio, with a -1 dBFS peak limiter.
- Mic gain, mute and RNNoise noise suppression.
- Monitor output so you hear tracks and captured apps yourself. Your own voice is not in the monitor.
- Library: import audio or video files (ffmpeg) and URLs (yt-dlp). Imports are loudness-normalized to -16 LUFS. Rename, tags, per-track volume, trim on a waveform, manual ordering.
- Stable track numbers. Deleting a track leaves a gap; numbers are never reused, so binds keep pointing at the same clip.
- Two-step playback, as in SLAM: select a track, then press the play key. The play key toggles. By default a second press pauses and the next press resumes; a setting changes it to stop-and-rewind.
- While running, PipeWire DJ becomes the system default microphone. The previous default is restored on exit, on SIGINT/SIGTERM, and after a crash with `pipewire-dj --restore`.
- Global shortcuts through the XDG desktop portal (Hyprland, KDE, GNOME) and a CLI for any compositor.

## Requirements

PipeWire 1.x with `pw-metadata`, WebKitGTK 4.1, `ffmpeg`, `yt-dlp` (only for URL imports), Rust 1.90+ and Node 20+ to build.

## Build and install

```sh
make install   # builds, then installs ~/.local/bin/pipewire-dj and a .desktop entry
make test      # Rust unit tests
```

The installed `.desktop` file matters: its name (`dev.pipewire_dj.PipeWireDJ`) is the app id the desktop portal requires before it allows global shortcuts.

## Counter-Strike 2

CS2 no longer accepts commands from outside the game. `-netconport` is gone, and `-concommandpipe` requires `-insecure`, which disables VAC. Communication therefore only runs from the game to the app:

1. PipeWire DJ writes `dj.cfg`, `dj_list.cfg` and `dj_state.cfg` into `game/csgo/cfg/`.
2. The `dj_*` aliases `echo` marker lines. CS2 writes its console to a log file, and the app tails that file (about 50 ms latency).
3. While the app runs, `dj_state.cfg` sets `voice_vox 1` (open mic). The app gates what is audible: your voice only while you hold push-to-talk, and the track while it plays. Your push-to-talk key is rebound to `+dj_ptt`, which still runs `+voicerecord` and also tells the app.
4. When the app exits it writes `voice_vox 0`. CS2 runs `dj_state` on every push-to-talk release, so it returns to normal push-to-talk at the next key press.

Setup:

- Steam launch options: add `-con_logfile pipewire_dj.log`.
- Add `exec dj` to `game/csgo/cfg/autoexec.cfg`.
- In CS2, set the voice input device to PipeWire DJ, or leave it on the system default.

Console commands:

| Command | Effect |
|---|---|
| `dj_list` | Print the track list |
| `dj_<number>`, `dj_<tag>` | Select a track |
| `dj_play` (bound to the play key, default `kp_enter`) | Play / pause |
| `dj_stop`, `dj_restart` | Stop, or restart from the trim start |

A track can have its own bind key that selects it, or plays it at once if "instant" is set.

## CLI

```sh
pipewire-dj toggle | stop | restart
pipewire-dj load <n|tag> | play <n|tag>
pipewire-dj ptt-on | ptt-off | mute
pipewire-dj import <file|url> | list | status | show
pipewire-dj --restore     # reset the default microphone after a crash
```

On Hyprland, portal shortcuts are bound like `bind = , F9, global, dev.pipewire_dj.PipeWireDJ:toggle`. Using `exec, pipewire-dj toggle` also works.

## Data

Settings, the library index and converted tracks live in `~/.local/share/pipewire-dj/`. Tracks are stored as raw 48 kHz stereo float32 files (`tracks/<id>.f32`).

## Known limitations

- The virtual source uses `media.class = Audio/Source`. `Audio/Source/Virtual` crashes the audioconvert plugin for client streams on PipeWire 1.6.8.
- Only one game adapter (CS2) exists so far. The `games` module is where others go.
- Not yet tested in a live CS2 match. In particular, how quickly CS2 flushes the console log during gameplay, which sets push-to-talk latency, still needs checking.

## License

GPL-3.0-or-later.
