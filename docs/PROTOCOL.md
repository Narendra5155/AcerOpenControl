# aocd socket protocol (API version 1)

`aocd` listens on `/run/acer-open-control/aocd.sock`, a Unix stream socket. Each
request is one JSON object on one line; each reply is one JSON object on one
line. A connection may carry any number of requests.

```sh
echo '{"cmd":"status"}' | socat - UNIX-CONNECT:/run/acer-open-control/aocd.sock
```

Every reply has `"ok": true|false`. Failures carry `"error": "message"`.
Requests refused for permission reasons also carry `"code": "forbidden"`.

**Permissions:** read requests (`status`, `get_*`) are open to everyone.
Requests marked ✎ need root or membership in `sudo`, `admin`, `wheel` or
`acer-open-control`.

**Read-only mode:** when no model profile matches, every ✎ request fails
with an error that starts `read-only:`.

## Read requests

### `{"cmd": "status"}`
```json
{"ok": true, "api_version": 1, "version": "0.2.0",
 "model": "acer-aspire-a715-79g", "read_only": false,
 "mode": "balanced", "cpu": 72.0, "gpu": 61.0, "gpu_asleep": false,
 "target": 54, "bios_control": false,
 "fans": [{"label": "CPU", "duty": 54, "rpm": 2356}, {"label": "GPU", "duty": 54, "rpm": 2227}],
 "emergency": false, "fault": null,
 "features": {"available": {"keyboard": true, "keyboard_rgb": true, "perf_profile": true,
                            "battery_limit": true, "webcam": true},
              "kbd": {"r": 255, "g": 96, "b": 0, "brightness": 180},
              "perf_profile": 3, "charge": {"enabled": true, "start": 75, "stop": 80},
              "webcam": true},
 "hotkey": {"code": 131, "action": "kbd-toggle"}}
```

| Field | Meaning |
|---|---|
| `cpu`, `gpu` | °C, or `null` if unavailable |
| `gpu_asleep` | `true` when the dGPU is runtime-suspended; `gpu` is then `null` |
| `target` | duty % the daemon is sending, or `null` |
| `bios_control` | `true` when the EC's own logic drives the fans |
| `features` | refreshed about every 7.5 s and after each change; `kbd` is the last setting made through aocd (the firmware can't report it) |
| `hotkey` | last Fn hotkey seen |

### `{"cmd": "get_model"}`
The matched profile summary: `model` (`id`, `name`, `platform`, `status`,
`notes`) or `null`; `path`; `machine` (DMI strings); `capabilities` (`fans`,
`fan_labels`, `keyboard` = `"rgb-single-zone"`|`"white"`|`null`,
`keyboard_max_brightness`, `perf_profile`, `battery_limit`, `webcam`, `hotkeys`);
`perf_profiles` (list of `index`, `key`, `label`, `description`); `hotkeys_enabled`.

### `{"cmd": "get_config"}`
`config` (the full configuration, as in [USAGE.md](USAGE.md#configuration-file)),
`modes` (the list of mode names), `presets` (`{"silent": [[t, d], ...], ...}`).

### `{"cmd": "get_features"}`
Same object as `status.features`, refreshed now.

## Change requests (✎)

| Request | Notes |
|---|---|
| `{"cmd": "set_mode", "mode": "silent"}` | `auto`, `silent`, `balanced`, `performance`, `max`, `custom` |
| `{"cmd": "set_curve", "points": [[50, 0], [90, 100]], "activate": true}` | validated; `activate` (default `true`) switches to `custom` |
| `{"cmd": "set_kbd", "r": 255, "g": 0, "b": 0, "brightness": 200}` | any subset of fields; brightness-only requests don't resend the colour |
| `{"cmd": "set_perf_profile", "profile": "performance"}` | a key or an index; `"bios"` or `null` stops re-applying at boot |
| `{"cmd": "set_charge_limit", "enabled": true, "start": 75, "stop": 80}` | stop 50–100, start 40 to stop−1 |
| `{"cmd": "set_webcam", "on": false}` | |
| `{"cmd": "reapply"}` | re-register hotkeys, reapply saved features, resend fan duty (used after resume) |

The feature setters reply with the refreshed `features` object (plus `"ok": true`).

## Compatibility

`api_version` increases only on incompatible changes. New fields and new
requests may be added within version 1; clients should ignore what they don't
know.
