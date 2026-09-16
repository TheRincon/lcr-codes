# §6.4 border position sequences

Cyclic binary sequences over the reference-border ring, one per version, in which
every window of `k` consecutive cells is unique. The luma layer detection reads
(§12.1); colour rides on top for the §12.3 references.

`min dist` is the smallest Hamming distance between any two windows — what a
single misread cell costs. `max run` is the longest stretch of one luma, capped at
4 so the ring always carries module pitch.

| v | side | ring positions | k | unique | min dist | max run | balance |
|---|---:|---:|---:|:--:|---:|---:|---:|
| 1 | 21 | 104 | 8 | yes | 1 | 4 | 49% |
| 2 | 25 | 120 | 8 | yes | 1 | 4 | 48% |
| 3 | 29 | 136 | 9 | yes | 1 | 4 | 50% |
| 4 | 33 | 152 | 9 | yes | 1 | 4 | 49% |
| 5 | 37 | 168 | 9 | yes | 1 | 4 | 50% |
| 6 | 41 | 184 | 9 | yes | 1 | 4 | 50% |
| 7 | 45 | 200 | 9 | yes | 1 | 4 | 49% |
| 8 | 49 | 216 | 9 | yes | 1 | 4 | 50% |
| 9 | 53 | 232 | 9 | yes | 1 | 4 | 50% |
| 10 | 57 | 248 | 9 | yes | 1 | 4 | 51% |
| 11 | 61 | 264 | 10 | yes | 1 | 4 | 48% |
| 12 | 65 | 280 | 10 | yes | 1 | 4 | 50% |
| 13 | 69 | 296 | 10 | yes | 1 | 4 | 50% |
| 14 | 73 | 312 | 10 | yes | 1 | 4 | 49% |
| 15 | 77 | 328 | 10 | yes | 1 | 4 | 48% |
| 16 | 81 | 344 | 10 | yes | 1 | 4 | 49% |
| 17 | 85 | 360 | 10 | yes | 1 | 4 | 48% |
| 18 | 89 | 376 | 10 | yes | 1 | 4 | 50% |
| 19 | 93 | 392 | 10 | yes | 1 | 4 | 48% |
| 20 | 97 | 408 | 10 | yes | 1 | 4 | 49% |
| 21 | 101 | 424 | 10 | yes | 1 | 4 | 49% |
| 22 | 105 | 440 | 10 | yes | 1 | 4 | 50% |
| 23 | 109 | 456 | 10 | yes | 1 | 4 | 48% |
| 24 | 113 | 472 | 10 | yes | 1 | 4 | 49% |
| 25 | 117 | 488 | 10 | yes | 1 | 4 | 49% |
| 26 | 121 | 504 | 10 | yes | 1 | 4 | 50% |
| 27 | 125 | 520 | 11 | yes | 1 | 4 | 50% |
| 28 | 129 | 536 | 11 | yes | 1 | 4 | 50% |
| 29 | 133 | 552 | 11 | yes | 1 | 4 | 49% |
| 30 | 137 | 568 | 11 | yes | 1 | 4 | 51% |
| 31 | 141 | 584 | 11 | yes | 1 | 4 | 51% |
| 32 | 145 | 600 | 11 | yes | 1 | 4 | 50% |

All 32 versions generated and verified: every cyclic window is distinct, no run
exceeds 4, and the checks are brute force over the extracted windows
rather than an appeal to the construction.

## Minimum window distance is 1, and that is not the number to judge this on

Every version above reports `min dist` 1: some pair of windows differs in a single
cell. That is not a flaw in the search, it is sphere-packing. Separating `p`
windows by distance 3 needs `p·(1+k) ≤ 2^k`, which at v1 means a window of **12**
cells rather than 9, and at v32 **15** rather than 12. The constraint is real and
the cost is a longer readable fragment.

But a decoder does not read exactly `k` cells and look them up. It reads whatever
arc of the ring is not obscured and correlates it against all `p` rotations. A
single misread cell then costs nothing as long as the arc is long enough for the
true rotation to win outright — which is what the table below measures, by
corrupting arcs and checking the correct position still wins.

| v | ring positions | k | clean | 1 misread | 2 misreads | worst case, as % of ring |
|---|---:|---:|---:|---:|---:|---:|
| 1 | 104 | 8 | 8 | 15 | 20 | 19.2% |
| 5 | 168 | 9 | 9 | 17 | 22 | 13.1% |
| 12 | 280 | 10 | 10 | 18 | 22 | 7.9% |
| 32 | 600 | 11 | 11 | 19 | 23 | 3.8% |

Measured over 60 randomised trials per arc length, the true position required to
win outright — ties count as failures. Read the last column as the answer to "how much of the ring does glare have to leave me?"

## v1 worked example — 104 cells, window 8

Luma sequence, the layer §12.1 detects from:

```
DLDDLDLDDLLLDLLLDLLDDDLDLDDDDLLDLDLLLDLDDDLLDLLDDLLLDDDLLDDDDLLLLDDDDLDD
DLDDLLDDDLLLDLDLLDLLLDDLLDDLDLDL
```

Colour assignment for `C1`, counted by border-cycle index (0–1 dark hues, 2–3 light hues, 4 white, 5 black):

```
[18, 18, 17, 17, 17, 17]
```

Every border colour still appears, which is what §12.3 needs from the ring — the sequence changes the *order*, not the coverage.
