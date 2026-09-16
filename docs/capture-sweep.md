# Capture sweep — measuring the real working range

*Written 2026-09-08. The follow-up to the first real-world decode of 2026-09-04, which
proved a phone could read a symbol off a screen and established nothing else.*

Every operating-envelope figure in this project — `just envelope`, `just profiles`, D10's
rate validation, D11's `C3` redesign — comes from the §15 degradation harness, which is
code. The harness has been wrong twice (the quiet-zone clipping and the point-sampled
resampler), and both times the numbers downstream of it were wrong too and looked fine.

This procedure produces the first numbers that do not come from it: **success rate against
capture geometry, measured through real optics.** It exists to be run before the wire
format freezes, because a format frozen against a simulator is frozen against a guess.

---

## 1. What gets measured, and in what units

Not centimetres and not degrees. Both of those describe where the operator stood, which is
not what the decoder experiences, and neither survives a change of phone, panel, or symbol
size.

**Distance is recorded as captured module pitch** — sensor pixels per module, read off the
fitted homography (`Transform::pitch_range`). A symbol of fixed physical size subtends
fewer sensor pixels the further away the camera goes, so pitch *is* distance, expressed in
the one unit that transfers between rigs and that §13.1's floor is already stated in.

**Angle is recorded as obliquity** — the coarsest module pitch over the finest, across the
symbol. A projective map compresses the far edge more than the near one, so a face-on view
reads 1.0 and the ratio climbs as the phone tilts. Converting it to degrees needs the
camera's focal length, which a browser does not expose and a phone does not report, so the
raw ratio is what gets written down.

Both come out of the decoder for free, so **the rig measures itself**. There is no tape
measure and no protractor, and the operator's job is only to move slowly and cover the
range.

### Failures carry geometry too

This is the part that makes the data analysable. A failed `decode` returns an error
string and nothing else, so a failed trial could not be binned by the conditions it failed
under — and the failures are the entire point of a sweep.

`decoder.probe` (added for this, §12.1–§12.3 only) runs detection and the format read and
stops. So a frame that was *found but not decoded* still reports its pitch and obliquity,
and only a frame where nothing was located at all is logged without them. Those are
different limits and the report separates them:

- **found, not decoded** — in range, out of margin. The payload is the binding constraint.
- **not found** — out of range entirely. Detection is the binding constraint.

---

## 2. Setting up

```bash
just web          # builds, transpiles, serves on :8000
```

Then, from another terminal, expose it so the phone can reach it over HTTPS — camera
access needs a secure context and `http://192.168.x.x` will not do:

```bash
cloudflared tunnel --url http://localhost:8000
```

| Device | Page | Role |
|---|---|---|
| Laptop | `/display.html` | Presents the symbol |
| Phone | `/sweep.html` | Decodes and records |

### 2a. Laptop OS settings

These are §13.1's conforming-display requirements. Skipping them measures the wrong thing.
Paths are macOS; the equivalents exist under any OS's display settings.

| Setting | Where (macOS) | Why |
|---|---|---|
| Brightness → **maximum** | System Settings → Displays → slider | §13.1 point 1. Also what moves most OLED panels from PWM to DC dimming, which is what actually removes rolling-shutter banding. |
| "Automatically adjust brightness" → **off** | System Settings → Displays | Otherwise the panel dims part-way through a run and the luma drifts under you. §13.1 names auto-brightness as a moderate impairment for exactly this reason. |
| Night Shift → **off** | System Settings → Displays → **Night Shift…** | Warms the whole palette. Shifts it further than any print process does. |
| True Tone → **off** | System Settings → Displays → **True Tone** checkbox | Adapts colour to ambient light, so the palette moves when the room does. |
| Colour profile → sRGB or the display default | System Settings → Displays → **Color Profile** | Vendor "Vivid"/"Saturated" presets push the palette outside what the decoder expects. |

Then **wipe the screen** — fingerprint smear is a common and trivially fixed cause of glare
failure, and §13.1 says to prompt for it.

### 2b. Phone OS settings: none

The phone's own Night Shift and True Tone change what *its screen* shows. They do not touch
what the camera sensor records, which is all this measures. There is nothing to set.

### 2c. Page controls — not OS settings

Two of the things this document used to list alongside the settings above are checkboxes in
our own pages. Both already default correctly, so in normal use you only touch them for the
deliberate A/B runs.

| Control | Page | Device | Default | When to change it |
|---|---|---|---|---|
| **Combine frames (§12.4)** | `sweep.html` | phone | **off** | Only as its own labelled run. Accumulation makes success a function of how long you lingered rather than where you stood — §12.4's accumulator is right for a real scanner and wrong for a measurement. The mode is recorded either way. |
| **Reference border (§6.4)** | `display.html` | laptop | **on** | Only for run 4's A/B. On is the conforming screen default (D14). |

### 2d. Starting

**Laptop:** pick a profile and payload in `display.html`, press **Present full screen**, and
leave the impairment sliders at zero — the phone's real optics supply the impairments.

**Phone:** fill in the session panel once (camera, panel, ambient light, and the display
px/module you set on the laptop), then **Start recording**. Profile and version are read
from the symbol, so they are not asked for.

---

## 3. The runs

Four, in this order. Name each one in the run field before starting it; the report groups
by run and the live bin table resets its view to the current one.

### Run 1 — `distance-c1`

Profile `C1`, 12 px/module, face-on. Start close enough to decode comfortably, then **walk
slowly backwards** until the phone stops finding the symbol at all, then walk slowly back
in. Repeat two or three times.

Slowly is the whole technique: each 120 ms tick is one Bernoulli trial at one geometry, so
walking back is what fills the bins. Watch the live table and stand still for a moment
wherever a bin is still dim — under 10 trials it is not yet evidence.

The number this produces is the working range: the finest pitch that still decodes ≥ 95%
of the time.

### Run 2 — `distance-c3`

Identical, profile `C3`. This is the comparison PROGRESS asks for. `C3` carries 60% of its
payload in chroma (D11), and chroma's handicap is spatial bandwidth rather than noise
(D10), so the prediction is that **`C3` gives up first as pitch falls** — its chroma cells
run out of sensor pixels before `C1`'s do. If it does not, D10's account of why the rate
offset is necessary needs revisiting.

### Run 3 — `angle-c1`

Back to `C1`, at a pitch comfortably inside run 1's ≥ 95% range — pick a distance and hold
it. Now tilt: sweep the phone through increasing off-axis angles, in several azimuths
(from the side, from above, from a corner), until decoding stops.

Azimuth matters and is worth deliberately varying. The harness found a real spread across
azimuth at v4, and the cause was structural: three finders and only one alignment marker,
so tolerance genuinely depends on which corner the projection foreshortens. This is the
first chance to see whether that shows up through real optics.

**Aim by the obliquity readout, not by an angle.** §13.2 recommends 10–20° off-axis to move
the phone's own reflection off the glass, but a fixed angle produces different obliquity at
different distances, and sweep 4 showed what that costs: the operator held a careful 10°,
landed at obliquity 1.05–1.18, and decoded worse than the run before.

Pooled over 1,549 `C1` trials from sweeps 1–4, decode rate by obliquity:

| obliquity | 1.00–1.05 | 1.05–1.10 | 1.10–1.20 | 1.20–1.25 | 1.25–1.40 | **1.40–1.60** | 2.00+ |
|---|---:|---:|---:|---:|---:|---:|---:|
| rate | 5% | 37% | ~40% | 58% | ~71% | **84%** | 0% |

The shape survives holding pitch constant, so it is tilt and not distance. **Target
1.30–1.50** — the page now says so live and warns past 1.90, where foreshortening starts
costing more than the reflection did.

### Run 4 — `glare-c1`

Same distance, face-on, but position the phone so a light source reflects off the panel and
into the symbol. Sweep the reflection across it, including over a finder — the pathological
case §15 calls out. This is the impairment §3 names as dominant on screens, and the one the
§6.4 reference border and §12.3's robust refit exist to defeat.

If you want the border's contribution, run it twice with the display's **Reference border**
checkbox on and off. That is a cheap A/B and nothing in the project has measured it against
real glare.

---

## 4. Reading the result

Export from the phone (**Copy JSON**, or **Download**), get the file onto the laptop, and:

```bash
just sweep path/to/sweep-20260908T103000.json
```

The report gives, per run and per profile: success rate binned by captured pitch, success
rate binned by obliquity, a stated working range, and the share of frames where nothing was
found at all.

**What to do with it.** Compare against `just envelope`, which reports the same metric from
the simulator. Where they disagree the harness is what is wrong — it is the model, and this
is the measurement. Two disagreements worth looking for specifically:

- **The pitch floor.** §13.1 mandates ≥ 8 *display* pixels per module and D6 records that
  the binariser once failed at exactly that. The captured-pitch floor is a different number
  and open decision #14 wants it measured.
- **`C1` against `C3`.** `just profiles` compares them in simulation at v4. If the real
  ordering differs, the profile guidance in §5 is resting on the harness.

Write the resulting figures into `PROGRESS.md` with the log committed alongside them, so
the next person can tell which numbers came from optics and which from code. That
distinction is the reason for the exercise.

---

## 5. Known limitations of this procedure

Worth stating, because the failure mode of a first measurement is being quoted later as
more than it was.

- **One phone and one panel is one sample.** PenTile against RGB-stripe, OLED against LCD,
  and a wide-angle against a telephoto module will all move these numbers. The procedure
  transfers; the figures do not.
- **Ambient light is uncontrolled** and only recorded as a free-text note. A sweep run at
  night and one run under office overheads are not directly comparable.
- **Obliquity conflates tilt with the pitch gradient.** It is a monotone proxy for viewing
  angle, not the angle, and at very low tilt it is dominated by fit noise rather than by
  geometry.
- **Motion is not held constant.** Walking introduces motion blur that a posed capture
  would not, so these figures are, if anything, pessimistic against a user standing still.
  That is the right direction to be wrong in, but it is a bias and not a bound.
- **Nothing here measures moiré**, which needs a deliberate sweep of fractional
  device-pixel ratios (§15, open #14) rather than a walk backwards.
