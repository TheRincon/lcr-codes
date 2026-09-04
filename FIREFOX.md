# Running the LC Code scanner in Firefox

*Written 2026-09-03. The counterpart to [`WASM-PLAN.md`](WASM-PLAN.md), which covers the
opposite case — a host you control, with no JavaScript anywhere.*

This is the **stock-browser** path: Firefox as shipped, camera capture, decoding on-device
with nothing uploaded. It applies almost unchanged to Chrome and Safari; the places they
differ are called out.

**Status: the JS and wasm path is verified, the browser UI is not.** The transpiled module
was exercised end to end under Node — single-frame decode, the multi-frame accumulator, and
resource disposal — against a real rendered symbol. The page in `web/` was written against
that verified API but has not itself been opened in a browser here. Treat §5 as the list of
things to check on first run.

---

## 1. Quick start

```bash
just web            # builds, transpiles, generates a sample frame, serves on :8000
```

Then open **`http://localhost:8000/`** in Firefox and either press *Decode sample frame*
(no camera needed) or *Start camera*.

`just web` needs Node 18+. Note that `nvm` may be shadowing a newer Node with an old one —
if `npm --version` reports `Cannot find module 'node:path'`, that is the symptom. The recipe
forces Homebrew's Node onto `PATH` for exactly this reason, so it works regardless, but
your interactive shell may still be on the old one.

---

## 2. What is actually happening

Firefox has no Component Model support — no browser does, and none is close to shipping it.
So the component is **transpiled**:

```
dist/lc-code-scanner.wasm          the component, 193 KB
        │
        │  jco transpile
        ▼
web/lc/lc-code-scanner.core.wasm   plain core wasm, 184 KB
web/lc/lc-code-scanner.js          Component Model ABI implemented in JS, 107 KB
```

The JS half is not glue you could hand-write away — it is the canonical ABI: lifting and
lowering records, strings and lists across linear memory, plus the resource table backing
the `accumulator`. Firefox runs it as an ordinary ES module.

**Size, gzipped — the figure that matters over a network:**

| | raw | gzip |
|---|---:|---:|
| `lc-code-scanner.core.wasm` | 187,955 B | 75,101 B |
| `lc-code-scanner.js` | 109,201 B | 21,285 B |
| **total** | 297,156 B | **96,281 B** |

That is **47% of §14.2's 200 KB decode-only budget**, which the spec states as gzipped
precisely because it was written about this case. D8 set that budget aside when the target
became a component loaded from disk; in a browser the original measure applies again, and
there is a lot of headroom.

---

## 3. Firefox specifics

### The camera needs a secure context

`getUserMedia` is refused outside a secure context. In practice:

| origin | camera |
|---|---|
| `http://localhost:8000` | ✅ works |
| `http://127.0.0.1:8000` | ✅ works |
| `http://192.168.1.x:8000` | ❌ **refused** — page loads, camera does not |
| `https://…` | ✅ works |

This is the single most common way the demo appears broken. The page still loads and the
sample-frame button still works, so it looks like a camera bug rather than an origin
problem. **To test on a phone you need HTTPS** — see §4.

For local experimentation only, Firefox exposes a pref that relaxes this. In `about:config`:

```
media.devices.insecure.enabled       = true
media.getusermedia.insecure.enabled  = true
```

Set them back afterwards. They apply to every site, not just yours, which is why this is a
scratch-machine measure and not advice for anyone else's browser.

### The `.wasm` MIME type must be `application/wasm`

The generated loader calls `WebAssembly.compileStreaming`, which **rejects any response
whose `Content-Type` is not `application/wasm`** — deliberately, and with an error that
names the type it got.

`just web` serves through `python3 -m http.server`, and this was verified against the
Python in use here (3.14, which returns `application/wasm` correctly). Older Pythons do not
know the type and will send `application/octet-stream`, which fails. If you hit that,
either serve with something else or register the type first:

```bash
python3 -c "import mimetypes; mimetypes.add_type('application/wasm', '.wasm')" # in a wrapper
# or simply:
npx --yes serve web
```

Chrome and Safari enforce the same rule.

### `file://` will not work

The page is an ES module and fetches its own wasm, so opening `web/index.html` directly
from disk fails on module CORS before it reaches anything interesting. It must be served.

### JSPI absence is already handled — do not "fix" it

The generated JS calls `WebAssembly.promising`, which is JavaScript Promise Integration.
**Firefox does not ship JSPI**, and every one of those calls is wrapped:

```js
try   { realloc0Async = WebAssembly.promising(exports1.cabi_realloc); }
catch { realloc0Async = exports1.cabi_realloc; }
```

So Firefox takes the synchronous path silently and correctly. LC Code's interface has no
async functions, so nothing is lost. This is worth knowing because `WebAssembly.promising`
appearing in a bundle looks like a compatibility bug and is not one.

### `Symbol.dispose` absence is also handled

jco emits `Symbol.dispose || Symbol.for('dispose')`, so the `accumulator`'s disposal method
exists on engines without Explicit Resource Management. The demo calls it through optional
chaining. **Do dispose it** — the accumulator holds guest-side state, and dropping the JS
handle without disposing leaks it inside the wasm instance for the life of the page.

---

## 3a. Laptop displays, phone scans

`just web` serves two pages:

| page | device | needs |
|---|---|---|
| `/display.html` | laptop | encodes JSON to a symbol and draws it at exact device pixels |
| `/` | phone | camera, decode, and the payload rendered back as JSON |

Both load the **full** component rather than the scanner build. `display.html` obviously
needs `encoder`; `index.html` needs `codec.cbor-to-json`, because §9.4 puts CBOR on the
wire and a phone showing a payload wants the JSON the sender authored. That function is
kept out of the minimal scanner deliberately: §9.4's JSON writer and the CBOR value tree
cost ~16 KB raw, which is 8% of §14.2's decode-only budget for a display convenience a
shipping scanner can do host-side. Over a network it is ~4 KB gzipped and would hardly
register — the two measures disagree, which is the open question in §14.2 / decision #16.

The display page has a **Present full screen** button that blanks everything except the
symbol on white. Use it: §13.1's requirements are the sender's, and they dominate the error
budget. Turn the laptop to maximum brightness and switch off Night Shift / True Tone before
scanning; a warm-shifted display moves the palette further than any print process does.

### iPhone specifically

**Every browser on iOS is WebKit**, including Firefox for iOS — the engine is Safari's
regardless of the icon. So test in whichever you like, but the behaviour and the
constraints are Safari's:

* **HTTPS is mandatory.** iOS refuses `getUserMedia` outside a secure context with no
  override — there is no `about:config` equivalent. §4 covers the options; a tunnel is the
  path of least resistance.
* **The camera prompt appears once per origin.** A tunnel that issues a fresh hostname each
  run re-prompts every time; a stable hostname does not.
* `facingMode: environment` selects the rear camera, which is the one you want — better
  optics and no mirroring.
* Safari applies its own capture processing. If decoding is marginal, try moving slightly
  rather than holding still and hoping: the accumulator combines frames, so small movements
  between frames help rather than hurt.

Firefox for Android enforces the same secure-context rule, so pick one:

1. **A tunnel** — `cloudflared tunnel --url http://localhost:8000` or the ngrok equivalent.
   Gives a real HTTPS origin in one command, and is the least trouble.
2. **A local HTTPS server** with a self-signed certificate, then accept the warning on the
   phone. Firefox for Android will let you proceed.
3. **USB port forwarding** so the phone sees the server as `localhost`, via
   `about:debugging` remote debugging.

Once it loads, the constraints that matter are physical rather than software: §13.1 wants
**≥ 8 device pixels per module**, so the symbol has to fill enough of the frame. A v4 symbol
is 33 modules across, which at 8 px/module needs the symbol to span at least ~264 px of the
captured frame. Too far away and it fails for want of resolution, not want of focus.

---

## 5. If it does not work, check in this order

| symptom | almost certainly |
|---|---|
| Blank page, console shows a module error | Opened over `file://`, or the server is not running |
| `compileStreaming: Incorrect response MIME type` | `.wasm` served as `application/octet-stream` — §3 |
| Page loads, *Start camera* errors immediately | Not a secure context — §3 |
| Camera runs, never decodes | Symbol too small in frame, or below 8 px/module — §4 |
| Decodes but body is gibberish | Wrong `stride`/`format` in the `image` record |
| Works once, then slows over minutes | `accumulator` not disposed — §3 |

The page surfaces `snr-db`, `chroma-recovered`, `references-rejected` and `alignment-found`
on every attempt, and they discriminate between these cases: a low SNR with
`alignment-found: false` is a framing or distance problem, while a healthy SNR with
`chroma-recovered: false` is a colour problem — glare, or a display colour mode left on.

---

## 6. Performance

A 492×492 frame decodes in **63 ms** measured under Node on this machine, which sets the
order of magnitude — a browser will be in the same range, a phone slower. The demo loop
therefore paces itself at ~8 attempts/second rather than running every animation frame;
decoding is synchronous and will block the main thread while it runs.

Two things worth doing if it feels sluggish:

* **Downscale before decoding.** A 1920×1080 capture costs several times a 640×480 one, and
  as long as the symbol still clears 8 px/module the extra resolution buys nothing.
* **Move it to a Worker.** The decode is pure computation over a byte buffer with no DOM
  access, so it ports to a Worker with no redesign, and the preview stops stuttering.

The accumulator is the other lever, and it is free: it is *cheaper* to combine several
mediocre frames than to demand one good one, because glare and banding move between frames
while the symbol does not (§12.4).
