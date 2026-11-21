<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="pics/monas/monas-mark-on-dark-512.png">
    <img src="pics/monas/monas-mark-on-light-512.png" alt="monas logo" width="75">
  </picture>
</p>

<h1 align="center">monasd</h1>

← [Back](README.md)

---

monasd is sdrtop with the screen taken off. Same engine, same radios, same
config file, but instead of panels it gives you a small HTTP API and a
WebSocket, and it runs until something tells it to stop. I wrote it because I
kept wanting the radio in the attic and the terminal on my laptop, and an SSH
session with tmux in it is a fine thing right up until the Wi-Fi blinks.

The three names, once, so the rest of this page makes sense: **Monas** is the
engine (the radio, the receivers, the measurements), **sdrtop** is the terminal
you already know, and **monasd** is Monas with no screen. sdrtop and monasd
are two programs built on the same engine. Neither knows the other exists.

**What it does today, honestly.** monasd opens the radio, computes the mode
you ask for (the same receivers sdrtop would run for that view), takes
commands, tells you how it is doing and streams events. Of the measurements
it serves the instrument's own readings so far: the radio with its gain
chain and frequency reference, the signal's figures, the I/Q front end, the
pipeline's vital signs, the recorder and the spectrum itself, each with the
same right to say "I don't know" that the screen has. The decoders (pagers,
FT8) and the 2.4 GHz band are next, so for now monasd is the bench without
the radio stations.

## Starting it

```sh
monasd
```

That opens the one radio it finds, computes the plain spectrum, and serves on
`127.0.0.1:7373`. The log goes to stderr, a line per line, so a terminal, a
pipe or the journal can take it. `Ctrl-C` or `SIGTERM` stops the stream and
writes the radio's settings back to the config, the same way `q` does in
sdrtop. On a terminal it opens with the Monas mark, the same one `--help`
starts with; a pipe or the journal is spared and gets only the log.

The flags are the ones that make sense without a screen:

```sh
monasd --device rtlsdr                 # pick the backend, as sdrtop's --device
monasd --frequency 433920000           # Hz, overrides the config
monasd --gain "LNA=24,VGA=30"          # or --lna / --vga, as in sdrtop
monasd --mode net_survey               # what to compute (below)
monasd --listen 0.0.0.0:7373           # where to serve (needs a token, below)
monasd --config /etc/sdrtop/config.toml
```

With more than one radio plugged in, monasd will not guess which one you
meant: there is nobody at a terminal to ask, so it refuses and lists them, and
`--device` says which. `monasd --help` has every flag, the modes, the API
and a few examples, and `man monasd` the same words in a man page. `-h` is the
short version, for when you only forgot one flag's name.

### The modes

`--mode` takes the name of the sdrtop view whose work you want done, so a
script and the person at the terminal mean the same thing by `net_survey`:

`spectrum` (the default), `net`, `net_survey`, `net_ble`, `net_census`,
`net_connection`, `net_coded`, `net_bt`, `net_piconet`, `net_bench`,
`wifi_survey`, `wifi_frames`, `wifi_airtime`, `radio_pocsag`, `radio_ft8`,
`lab_ft8_signal`, `lab_ft8_band`, `lab_ft8_receiver`.

A name it doesn't know is refused with that list. The Lab benches and the
compact views have no name of their own here because they ask for nothing
beyond the spectrum, and `spectrum` computes that. Your own layouts from
`~/.config/sdrtop/presets/` don't count: monasd doesn't read sdrtop's presets,
on purpose.

## The token

On loopback, anyone on the machine may talk to it, the same people who could
already run sdrtop there. Anywhere else, every request needs a token, because
a command retunes a radio somebody may be relying on, and that is not an
anonymous thing to do. monasd won't even start on `0.0.0.0` without one:

```toml
[monasd]
listen = "0.0.0.0:7373"
token  = "a long random string, not this one"
mode   = "net_survey"
```

and then every request carries it:

```sh
curl -H "Authorization: Bearer $TOKEN" http://attic:7373/v1/health
```

There is no TLS. On a network you don't trust, put it behind something that
does TLS, or behind SSH (`ssh -L 7373:127.0.0.1:7373 attic` and talk to
loopback), which is what I do.

## The API

Everything lives under `/v1`, so a later change of shape is a new prefix
rather than a surprise for every script written against this one. Eight
routes:

| Route | What it gives |
|---|---|
| `GET /v1/health` | The radio, whether it streams, whether its data is stale, the tuning, the mode, samples lost (the driver's and sdrtop's own, apart), each feed's refusals, the process's CPU and memory |
| `GET /v1/radios` | Every radio found at start, which one is open, and whether its support was tested on a real one or written from the library's headers |
| `GET /v1/config` | The engine's blocks of the config, as they would be saved now |
| `GET /v1/snapshot` | What the engine reads, group by group: the radio and its gain chain, the signal's figures, the I/Q front end, the pipeline's vitals, the recorder. Each figure a `value` or the reason it is `absent`, each group with its testimony: when it was read, whether it is stale |
| `GET /v1/spectrum` | The latest spectrum frame: every bin in dBFS, the axis they lie on and the resolution bandwidth they were measured in. Its own route, so a script that wants five numbers isn't handed eight thousand |
| `GET /v1/provenance` | Which radio, tuned where, in which mode, against what frequency reference, for how long: the same fields an export's header carries |
| `POST /v1/command` | One command, answered |
| `GET /v1/events` | A WebSocket, one JSON message per event |

**Commands** are JSON with a `command` field:

```sh
curl -X POST -H 'Content-Type: application/json' \
     -d '{"command":"tune","hz":433920000}' http://127.0.0.1:7373/v1/command
```

The answer is `200` with `{"reply":"applied","detail":"..."}` when it was done,
or `422` with `{"reply":"refused","reason":"..."}` when it wasn't, and then
nothing was changed. The commands are `tune` (`hz`), `sample_rate` (`hz`),
`gain` (`stage`, `db`), `gains` (`values`, front to back), `agc` (`on`),
`boost` (`on`), `reset`, `set_mode` (`section`, `view`), `stream` (`on`),
`dc_block` (`on`) and `device_option` (`id`, `choice`). A frequency the radio
doesn't tune, a rate outside its range or a stage it doesn't have is refused
with the reason. A gain between two of a stage's steps lands on the nearest
one, and the reply's `detail` says so, because a number that changed on its
way to the radio should never change silently.

**Events** arrive as `{"event": {...}, "lost_before": 0}`. The inner `event`
says what happened: `applied` (a command and its reply, whichever client sent
it), `log` (a log line), `gap` (samples went missing, with how many were the driver's and how
many ours) and `stale` (the radio stopped delivering, or started again). A
client that falls behind loses events rather than slowing the radio down, and
`lost_before` says how many it missed just before this one.

```sh
websocat ws://127.0.0.1:7373/v1/events
```

## Running it as a service

monasd is happy under systemd. A unit along these lines, for a user that is
in `plugdev` (or whatever group your udev rules hand the radio to):

```ini
[Unit]
Description=monasd, sdrtop's engine with no screen
After=network-online.target

[Service]
User=radio
SupplementaryGroups=plugdev
ExecStart=/usr/local/bin/monasd --config /etc/sdrtop/config.toml
Restart=on-failure

[Install]
WantedBy=multi-user.target
```

Two things to know. It writes the radio's settings back to that config when
it stops, so the user needs write access to the file. And `SIGTERM`, which is
how systemd stops things, is handled: the stream is stopped and the settings
are saved before it exits. The log lands in the journal (`journalctl -u
monasd`).

The config is the same file sdrtop uses, and the two share it politely:
monasd writes only the radio's blocks, never your layouts or your theme, and
nobody writes `[monasd]` but you. Who writes what is in
[Configuration](config.md#who-writes-what).

---

← [Back](README.md)
