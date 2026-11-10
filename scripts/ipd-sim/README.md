# Simulated polling signs

A PidgeIoT test harness: our own stand-in for a sign that polls a web data source, and not
any maker's software. Every page it serves says "simulated,
from your 2015 reference", and so does its `GetSignInfo` answer. The other SOAP answers carry
only what the WSDL defines. It exists so the fleet demo can run before
the loaned sample arrives.

One process plays N signs. Each sign has its own state directory and its own port. Each one:

- polls one URL at a set cadence, the way the LED manual's chapter 13 web data source does,
  with a Bearer (or Basic) token, a `User-Agent` and any custom headers;
- applies JSON (flat, or nested in dot notation) or `<DataItems>` XML to the data items it
  defines, and ignores names it does not define, as the manual says a sign does;
- keeps every value when the source is unreachable, answers a non-2xx or sends a body that
  does not parse;
- answers five SOAP actions at `POST /Soap1.wsdl` with the 2007 WSDL's names
  (`http://tempuri.org/action/SignSvr.<name>`): `AddUpdateLayout`, `AddUpdateMessage`,
  `UpdateDataItemValues`, `SendCommand` (119 Persist All, 106 Reload All) and `GetSignInfo`;
- keeps its layouts, messages and data items as XML files in the manual's chapter 21 shape;
- serves a status page at `/` that reloads every two seconds and highlights a value for ten
  seconds after it changes, and the same state at `/state.json`.

A control page stops and starts each sign. A start reads the state directory back, as a sign
reads its files at boot: data item values survive, and layout or message edits survive only if
`SendCommand 119` saved them.

Every port binds `127.0.0.1`. `--bind` changes that, and the SOAP and control ports have no
authentication, so do not.

Python 3 standard library only. It lives in `scripts/` beside the repo's other Python tools,
with its tests next to it as `scripts/test_purge_identities.py` is.

## Tests

```sh
python3 -m unittest discover -s scripts/ipd-sim
```

The tests open sockets on `127.0.0.1` only, on ports the kernel picks.

## Three signs against a local `wrangler dev`

1. Start the dev stack from the repo root, as `CLAUDE.md` describes: the docker-compose stack,
   then `cd dovecote && bunx wrangler dev --ip 127.0.0.1 --port 8787 --env dev`, then fancier.
2. In the dev dashboard at `http://127.0.0.1:4455`, create a flock with three pigeons.
3. Keep each device token in its own 0600 file as the token appears. This reads it from the
   terminal, so it never reaches the shell history or a config file. Paste the token, press
   Enter, then Ctrl-D:

   ```sh
   mkdir -p ~/.config/pidgeiot/ipd-sim
   (umask 077; cat > ~/.config/pidgeiot/ipd-sim/sign-a.token)
   ```

   A token can come from an environment variable instead (`token_env`). The simulator refuses a
   token file readable by group or others, and refuses a config that holds a token itself.
4. Copy `signs.example.json` outside the repo, for example to
   `~/.config/pidgeiot/ipd-sim/signs.json`. Put each pigeon's id in its URL and each token
   file's path in `token_file`. The state directory is created beside the config. Each sign polls
   `GET /device/pigeons/<id>/values?format=xml` every 60 seconds, the cadence the route's
   limit of 4 a minute per pigeon and address is sized around. Sign B sends its token as a Basic
   password under the user name `sign`; the other two send it as a Bearer token.
5. Start the signs and open the control page:

   ```sh
   python3 scripts/ipd-sim/ipd_sim.py --config ~/.config/pidgeiot/ipd-sim/signs.json
   # control page: http://127.0.0.1:8900/   signs: http://127.0.0.1:8901/ to :8903/
   ```

6. Give the flock data items in the dashboard: each `item.<name>` key of the flock's config is
   the data item `<name>`, so `{"item.lot.status": "OPEN"}` sets `lot.status`. A pigeon's own
   `item.` key overrides the flock's for that sign only. Within one poll, each sign's page shows
   the new values, highlighted, with the time each changed.
7. Stop one sign on the control page. Its port closes and its polls stop, so the dashboard shows
   it Stale about 6 minutes after its last poll and Offline about 31 minutes after.

The example config asks for `<DataItems>` XML (`"params": "format=xml"` and `"format": "xml"`),
the shape the manual names and the one to use for a sign: an XML name is a plain string, where
the sign's JSON reader may take the dot in `lot.status` as a path step (a bench test). List in
`data_items` the names the sign defines; a served name it does not define is ignored, as on a
real sign.
`docs/api.md` documents the route.

## Config

| Key | Default | Meaning |
|---|---|---|
| `control_port` | 8900 | the control page; 0 lets the kernel pick |
| `state_root` | `ipd-sim-state` | directory of the signs' state directories, relative to the config |
| `signs[].slug` | required | letters, digits and hyphens; names the state directory |
| `signs[].name` | the slug | shown on the page and in `GetSignInfo` |
| `signs[].port` | required | the sign's SOAP and page port; 0 lets the kernel pick |
| `signs[].height`, `width` | 32, 160 | reported by `GetSignInfo` |
| `signs[].data_items` | `[]` | the data items the sign defines; values for other names are ignored |
| `poll.url` | required | `http://` or `https://`; redirects are not followed |
| `poll.params` | none | the manual's "Additional parameter(s)", appended as a query string |
| `poll.format` | `json` | `json` or `xml` |
| `poll.every_secs` | 10 | seconds between polls |
| `poll.user_agent` | `sign-sim/1.0` | `""` sends no `User-Agent`, as libcurl does by default |
| `poll.auth` | `bearer` | `bearer`, `basic` (token as password, with `username`) or `none` |
| `poll.token_env`, `token_file` | one required | where the token comes from; a file must be 0600 |
| `poll.headers` | `{}` | extra headers; `Authorization` is refused here |

## State directory

Each sign's directory holds `layouts.xml` (`<Layouts>`), `messages.xml` (`<Messages>`),
`dataitems.xml` (`<DataItems>`) and `sim-state.json`, which holds what the simulator adds: when
each value last changed and the last poll's outcome. Delete a directory to reset that sign.

## What it leaves out

The 2007 WSDL's other 47 operations answer a SOAP fault that says "not simulated". Commands
other than 119 and 106 answer 0. There are no thresholds, conditions, zones as
their own records, schedules, images, snapshots, data source settings over SOAP, or the "Use ID
as prefix" option. Nothing is rendered: the page lists values, layouts and messages as text.
