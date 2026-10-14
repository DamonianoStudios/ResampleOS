"""Builds "Built-In Boom Bap", a demo project made only from ResampleOS's own sounds.

    python showcase/make_builtin_showcase.py

Every sound comes from the drum synth or the internal synths, every effect is built in,
and the loops use automation, so the project needs no samples or plugins and can ship with
the app. The project is written to showcase/built-in-boom-bap/; open it from File > Open
project.

88 BPM in A minor, five loops chained into a two-minute arrangement:
Intro (keys under a filter that opens) - Verse A - Verse B (lead, clap, delay throws) -
Break (filter sweeps up, drums drop out) - Verse A - Verse B - Outro (filter closes, the
master fades).
"""

import json
from pathlib import Path

OUT = Path(__file__).resolve().parent / "built-in-boom-bap"

PPQ = 480
BAR = PPQ * 4
STEP = PPQ // 4  # a sixteenth

# ---------------------------------------------------------------- pads


def cfg(kind, bus, vol=0.8, pan=0.0, **extra):
    """A pad's settings, every field spelled out as the app saves them."""
    base = {
        "kind": kind, "vol": vol, "pan": pan, "mute": False, "solo": False, "choke": 0,
        "bus": bus, "pitch": 0.0, "mode": "OneShot", "start": 0, "end": 0, "attack": 0.0,
        "release": 0.02, "reverse": False, "cutoff": 1.0, "reso": 0.0, "lfo_rate": 2.0,
        "lfo_depth": 0.0, "lfo_target": "Off", "lfo_shape": "Sine", "lfo_sync": 0,
        "target": "Synth",
        "synth": {"engine": "Sub808", "octave": -2, "decay": 0.5, "tone": 0.5, "drive": 0.3},
        "human_time": 0.0, "human_vel": 0.0,
    }
    base.update(extra)
    return base


def drum(engine, m, bus=1, vol=0.8, pan=0.0, vel=None, rnd=None, open_=False, key_lock=False, sync=0, **extra):
    patch = {
        "engine": engine, "m": m, "vel": vel or [0.0] * 8, "rnd": rnd or [0.0] * 8,
        "open": open_, "key_lock": key_lock, "sync": sync,
    }
    return cfg("Drum", bus, vol, pan, drum=patch, **extra)


def synth(engine, octave, bus, vol, decay, tone, drive, pan=0.0, **extra):
    return cfg("Midi", bus, vol, pan, synth={"engine": engine, "octave": octave, "decay": decay, "tone": tone, "drive": drive}, **extra)


HAT_DECAY_FEEL = [0, 0, 0, 0, 0, 0.35, 0, 0]  # harder hits ring a little longer
LOOSE = {"human_time": 0.25, "human_vel": 0.2}

# Bus 1 drums, Bus 2 keys and pad, Bus 3 lead and effects, Bus 4 bass.
PADS = [
    # (name, settings)                      pad 1 is bottom left, as on the matrix
    ("Kick", drum("Kick", [0.38, 0.45, 0.35, 0.4, 0.25, 0.15, 0.55, 0.4], vol=0.7, vel=[0, 0.3, 0, 0.5, 0, 0, 0, 0])),
    ("Snare", drum("Snare", [0.35, 0.65, 0.5, 0.0, 0.0, 0.6, 0.45, 0.35], vol=0.62, **LOOSE)),
    ("Clap", drum("Snare", [0.4, 0.4, 0.2, 0.75, 0.6, 0.4, 0.45, 0.1], vol=0.4, pan=0.1)),
    ("Rim", drum("Snare", [0.85, 0.1, 0.9, 0.0, 0.0, 0.7, 0.05, 0.4], vol=0.36, pan=-0.25, **LOOSE)),
    ("Closed Hat", drum("Metal", [0.5, 0.5, 0.2, 0.3, 0.45, 0.32, 0.5, 0.25], vol=0.36, pan=0.2, vel=HAT_DECAY_FEEL,
                        rnd=[0, 0, 0, 0, 0, 0.12, 0, 0.1], choke=1, **LOOSE)),
    ("Open Hat", drum("Metal", [0.5, 0.5, 0.2, 0.3, 0.45, 0.35, 0.45, 0.25], vol=0.32, pan=0.2, open_=True, choke=1)),
    ("Shaker", drum("Metal", [0.9, 0.9, 0.8, 0.1, 0.7, 0.4, 0.4, 1.0], vol=0.22, pan=-0.35, rnd=[0, 0, 0, 0, 0, 0.2, 0, 0.2], **LOOSE)),
    ("Conga", drum("Fm", [0.52, 0.067, 0.2, 0.2, 0.05, 0.0, 0.45, 0.5], vol=0.45, pan=0.3, **LOOSE)),
    ("Bass", synth("Sub808", -2, 4, 0.7, 0.45, 0.35, 0.35)),
    ("Keys", synth("Keys", 0, 2, 0.17, 0.65, 0.45, 0.2, **LOOSE)),
    ("Lead", synth("Square", 0, 3, 0.45, 0.35, 0.55, 0.15, pan=-0.1)),
    ("Pad", synth("Saw", 0, 2, 0.08, 0.8, 0.3, 0.1, pan=0.0, cutoff=0.55, attack=0.25, release=0.6)),
    ("808 Boom", drum("Kick", [0.28, 0.3, 0.5, 0.05, 0.15, 0.45, 0.9, 0.35], vol=0.6, key_lock=True)),
    ("Laser", drum("Fm", [0.6, 0.0, 0.9, 0.55, 0.3, 0.4, 0.4, 0.2], bus=3, vol=0.3)),
    ("Ride", drum("Metal", [0.35, 0.8, 0.5, 0.6, 0.35, 0.5, 0.85, 0.5], vol=0.25, pan=0.3, open_=True)),
    ("Tom", drum("Fm", [0.185, 0.067, 0.15, 0.3, 0.0, 0.0, 0.5, 0.3], vol=0.5)),
]
P = {name: i for i, (name, _) in enumerate(PADS)}

# ---------------------------------------------------------------- buses


def slot(kind, params=(0.0, 0.0, 0.0), sliders=(0.0, 0.0), on=True):
    return {"kind": kind, "params": list(params), "sliders": list(sliders), "on": on}


NONE = slot("None")


def bus(slots, level=1.0, pan=0.0, out=0, sc_src=0, sc_amount=-0.7, sc_release=0.18):
    return {"level": level, "pan": pan, "mute": False, "out": out, "slots": slots,
            "sc_src": sc_src, "sc_amount": sc_amount, "sc_release": sc_release}


BUSES = [
    # Drums: glued, then a little tape.
    bus([slot("Comp", (0.45, 0.5, 0.6)), slot("Cassette", (0.35, 0.15, 0.6)), slot("Eq", (0.55, 0.45, 0.55), (0.6, 0.5))], level=0.6),
    # Keys and pad: worn vinyl, a DJ filter for the automation to sweep, and a room.
    bus([slot("Vinyl", (0.4, 0.35, 0.45)), slot("Filter", (0.5, 0.25, 1.0)), slot("Reverb", (0.55, 0.5, 0.22))], level=0.5),
    # Lead and effects: an eighth-note tape echo for the throws, and a longer reverb.
    bus([slot("Delay", (0.55, 0.45, 0.22)), slot("Reverb", (0.65, 0.4, 0.25)), slot("Eq", (0.4, 0.5, 0.45), (0.6, 0.5))], level=0.85),
    # Bass: kept steady and ducked a touch under the drums.
    bus([slot("Comp", (0.5, 0.55, 0.7)), slot("Eq", (0.6, 0.45, 0.4), (0.6, 0.5)), NONE], level=0.75, sc_src=1, sc_amount=-0.35, sc_release=0.15),
    # The vocal bus, as a new project has it (unused here).
    bus([slot("Tune", (0.6, 0.5, 0.5), on=False), slot("VocalComp", (0.5, 0.4, 0.15)), slot("Throw", (0.0, 0.6, 0.4))]),
    # Master: gentle glue.
    bus([slot("Comp", (0.7, 0.3, 0.5)), NONE, NONE], level=0.9),
]
MASTER = 5

# ---------------------------------------------------------------- patterns


def note(start, length, pitch=60, vel=100):
    return {"start": start, "len": length, "pitch": pitch, "vel": vel}


def steps(positions, vel=100, bar=0, length=STEP):
    """Notes on sixteenth-note steps of a bar."""
    if isinstance(vel, int):
        vel = [vel] * len(positions)
    return [note(bar * BAR + s * STEP, length, 60, v) for s, v in zip(positions, vel)]


def pattern(bars, notes):
    return {"len_ticks": bars * BAR, "notes": sorted(notes, key=lambda n: n["start"])}


KICK = pattern(2, steps([0, 3, 8, 10], [118, 96, 112, 100]) + steps([0, 3, 8, 10, 14], [118, 92, 110, 98, 84], bar=1))
SNARE = pattern(2, steps([4, 12], 112) + steps([4, 12], 112, bar=1) + steps([15], 46, bar=1))
CLAP = pattern(2, steps([4, 12], 88) + steps([4, 12], 88, bar=1))
RIM = pattern(2, steps([6, 13], [72, 60]) + steps([7, 13], [64, 70], bar=1))
HATS_VEL = [104, 62, 90, 58, 100, 62, 88, 58]
HATS = pattern(2, steps(range(0, 16, 2), HATS_VEL) + steps([7], 44) + steps(range(0, 14, 2), HATS_VEL[:7], bar=1))
OPEN_HAT = pattern(2, steps([14], 92, bar=1))
SHAKER = pattern(1, steps(range(16), [60, 38, 50, 36] * 4))
CONGA = pattern(2, steps([5, 11, 14], [76, 64, 80]) + steps([2, 5, 11], [60, 74, 66], bar=1))
INTRO_HATS = pattern(1, steps(range(0, 16, 2), [80, 50, 70, 46] * 2))

# A minor: Am7 | Fmaj7 | Dm7 | Em7, voiced close around middle C.
CHORDS = [[57, 60, 64, 67], [57, 60, 64, 65], [57, 60, 62, 65], [55, 59, 62, 64]]
ROOTS = [33, 29, 38, 40]  # A1 F1 D2 E2


def keys_part():
    notes = []
    for b, chord in enumerate(CHORDS):
        for at, length, vel in [(0, 900, 96), (1200, 340, 74), (1560, 260, 66)]:
            notes += [note(b * BAR + at, length, p, vel) for p in chord]
    return pattern(4, notes)


def pad_part():
    notes = []
    for b, chord in enumerate(CHORDS):
        notes += [note(b * BAR, BAR - 60, p - 12, 70) for p in (chord[0], chord[2])]
    return pattern(4, notes)


def bass_part():
    # The 808 synth sounds two octaves below the note written.
    notes = []
    for b, root in enumerate(ROOTS):
        r = root + 24
        notes += [note(b * BAR, 600, r, 112), note(b * BAR + 720, 200, r, 88),
                  note(b * BAR + 1200, 420, r + 12 if b % 2 else r, 100), note(b * BAR + 1680, 200, r, 80)]
    return pattern(4, notes)


def lead_part():
    phrase = [  # (tick, length, pitch): A minor pentatonic, answering itself
        (0, 240, 76), (360, 120, 74), (480, 240, 72), (960, 480, 69),
        (BAR + 1200, 240, 67), (BAR + 1440, 480, 69),
        (2 * BAR, 240, 72), (2 * BAR + 240, 240, 74), (2 * BAR + 480, 480, 76), (2 * BAR + 960, 240, 79), (2 * BAR + 1200, 480, 76),
        (3 * BAR, 360, 74), (3 * BAR + 360, 240, 72), (3 * BAR + 720, 960, 69),
    ]
    return pattern(4, [note(t, length, p, 96) for t, length, p in phrase])


def once(bars, notes):
    return pattern(bars, notes)


CRASH = once(8, steps([0], 100))
TOM_FILL = once(8, [note(7 * BAR + s * STEP, STEP, p, v) for s, p, v in [(12, 64, 96), (13, 62, 92), (14, 60, 100), (15, 57, 108)]])
BOOM_DROP = once(4, steps([0], 120))
LASER_END = once(4, steps([14], 96, bar=3))

# ---------------------------------------------------------------- automation


def lane(target, points):
    return {"target": target, "points": [{"tick": t, "value": v} for t, v in points]}


def bus_param(b, s, p):
    return {"BusParam": {"bus": b, "slot": s, "param": p}}


KEYS_FILTER = bus_param(1, 1, 0)  # Bus 2's DJ Filter cutoff: 0.5 is open, lower is darker
LEAD_ECHO = bus_param(2, 0, 2)    # Bus 3's tape delay mix

INTRO_AUTO = [lane(KEYS_FILTER, [(0, 0.16), (3 * BAR, 0.42), (4 * BAR - STEP, 0.5)])]
VERSE_B_AUTO = [lane(LEAD_ECHO, [(0, 0.22), (BAR + 1400, 0.22), (BAR + 1440, 0.62), (2 * BAR - 1, 0.62), (2 * BAR + 240, 0.22),
                                 (7 * BAR + 1400, 0.22), (7 * BAR + 1440, 0.7), (8 * BAR - STEP, 0.7)])]
BREAK_AUTO = [lane(KEYS_FILTER, [(0, 0.5), (3 * BAR, 0.74), (4 * BAR - 2 * STEP, 0.8)]),
              lane({"PadVol": P["Pad"]}, [(0, 0.15), (4 * BAR - STEP, 0.3)])]
OUTRO_AUTO = [lane(KEYS_FILTER, [(0, 0.5), (4 * BAR - STEP, 0.12)]),
              lane({"BusLevel": MASTER}, [(0, 0.9 / 1.5), (2 * BAR, 0.9 / 1.5), (4 * BAR - STEP, 0.0)])]


def scene(name, bars, parts, automation=()):
    return {"name": name, "bars": bars, "patterns": [[P[pad], pat] for pad, pat in parts], "automation": list(automation)}


GROOVE = [("Kick", KICK), ("Snare", SNARE), ("Closed Hat", HATS), ("Open Hat", OPEN_HAT), ("Bass", bass_part()), ("Keys", keys_part()), ("Pad", pad_part())]
SCENES = [
    scene("Intro", 4, [("Keys", keys_part()), ("Pad", pad_part()), ("Closed Hat", INTRO_HATS), ("Laser", LASER_END)], INTRO_AUTO),
    scene("Verse A", 8, GROOVE + [("Ride", CRASH), ("Tom", TOM_FILL)]),
    scene("Verse B", 8, GROOVE + [("Lead", lead_part()), ("Clap", CLAP), ("Rim", RIM), ("Shaker", SHAKER), ("Ride", CRASH)], VERSE_B_AUTO),
    scene("Break", 4, [("Keys", keys_part()), ("Bass", bass_part()), ("Pad", pad_part()), ("Conga", CONGA), ("Rim", RIM),
                       ("808 Boom", BOOM_DROP), ("Laser", LASER_END)], BREAK_AUTO),
    scene("Outro", 4, [("Keys", keys_part()), ("Pad", pad_part()), ("Closed Hat", INTRO_HATS), ("808 Boom", BOOM_DROP)], OUTRO_AUTO),
]
CHAIN = [0, 1, 2, 3, 1, 2, 4]

# ---------------------------------------------------------------- the file


def main():
    first = {pad: pat for pad, pat in SCENES[0]["patterns"]}
    pads = []
    for i, (name, settings) in enumerate(PADS):
        pads.append({"index": i, "name": name, "cfg": settings,
                     "pattern": first.get(i, {"len_ticks": 2 * BAR, "notes": []}), "transients": [], "sample": None})
    project = {
        "version": 2, "bpm": 88.0, "swing": 0.57, "key": {"root": 9, "scale": "Minor"}, "banks": 2,
        "buses": BUSES, "conform_mode": "Smart", "auto_conform": False, "preserve_transients": True,
        "pads": pads, "fx_plugins": [], "scenes": SCENES, "scene": 0, "chain": CHAIN, "chain_on": True,
        "vocal_fx": {"gate_db": -80.0, "deess": 0.0, "duck_bus": 0},
    }
    OUT.mkdir(parents=True, exist_ok=True)
    (OUT / "project.json").write_text(json.dumps(project, indent=1), encoding="utf-8", newline="\n")
    bars = sum(SCENES[s]["bars"] for s in CHAIN)
    print(f"Wrote {OUT / 'project.json'}: {len(pads)} pads, {len(SCENES)} loops, {bars} bars ({bars * 4 * 60 / 88:.0f} s)")


if __name__ == "__main__":
    main()
