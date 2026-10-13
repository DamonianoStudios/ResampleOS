"""Builds the "Taking It Back" showcase project from samples in a local Splice library.

    python showcase/make_showcase.py [path to Splice folder]

The result is a ResampleOS project folder next to this script. Open it with
File > Open project, or start the app with the folder as its argument.

The samples are licensed to the Splice account that downloaded them and may not be
passed on as files, which is why the project folder is not committed to the repository.
"""

import json
import random
import shutil
import sys
from pathlib import Path

SPLICE = Path(sys.argv[1]) if len(sys.argv) > 1 else Path.home() / "Documents" / "Splice"
OUT = Path(__file__).parent / "taking-it-back"

BPM = 87.0
PPQ = 480
BAR = PPQ * 4
S16 = PPQ // 4

# pad number (1-16 on bank A) -> (name, file name in the Splice library)
SOUNDS = {
    1: ("Kick", "TAMUZ_SD_kick_dilla_heavy.wav"),
    2: ("Snare", "LEX_JBD_snare_boombap.wav"),
    3: ("Hat", "TAMUZ_TD_hat_swing.wav"),
    4: ("Open Hat", "TAMUZ_TD_hat_open_shorten_up.wav"),
    5: ("Rim", "TAMUZ_TD_rim_yummy.wav"),
    6: ("Clap", "TAMUZ_TD_clap_heavier_things.wav"),
    7: ("Ride", "TAMUZ_TD_ride_lush_tap.wav"),
    8: ("Conga", "TAMUZ_TD_conga_snap_hit.wav"),
    9: ("Keys", "PL_KU_87_Electric_Keys_Loop_Taking_It_Back_Am.wav"),
    10: ("Bass", "PL_KU_87_Electric_Bass_Guitar_Taking_It_Back_Am.wav"),
    11: ("Vibes", "PL_KU_87_Vibraphone_Loop_Taking_It_Back_Am.wav"),
    12: ("Gu Zheng", "PL_KU_87_Strings_Loop_Taking_It_Back_Gu_Zheng_Am.wav"),
    13: ("Crackle", "MARS_NOISE_fx_vinyl_crackle_analog_45rpm.wav"),
}
KICK, SNARE, HAT, OHAT, RIM, CLAP, RIDE, CONGA, KEYS, BASS, VIBES, ZHENG, CRACKLE, ANSWER = range(1, 15)

DRUMS, MUSIC, LOW, DUST = 1, 2, 3, 4  # buses


def note(bar, step, vel=100, length=S16, pitch=60):
    """A note at a 16th-note step (0-15) of a bar (0-based)."""
    return {"start": bar * BAR + step * S16, "len": length, "pitch": pitch, "vel": vel}


def on_the_one(*bars):
    return [note(b, 0, 110, BAR) for b in bars]


def beat(bars, seed, full=True, fill=True):
    """The drum parts for a loop of `bars` bars. `full` adds the ride and conga."""
    rng = random.Random(seed)
    parts = {p: [] for p in (KICK, SNARE, HAT, OHAT, RIM, CLAP, RIDE, CONGA)}
    for bar in range(bars):
        phrase = bar % 4
        last = fill and bar == bars - 1
        # The beat drops out for the second half of the last bar, then lands on the one.
        steps = range(8) if last else range(16)

        kicks = {0: [0, 7, 10], 1: [0, 3, 10], 2: [0, 7, 10], 3: [0, 6, 10, 11]}[phrase]
        parts[KICK] += [note(bar, s, 118 if s == 0 else 100) for s in kicks if s in steps]
        for s in (4, 12):
            if s in steps:
                parts[SNARE].append(note(bar, s, 120))
                parts[CLAP].append(note(bar, s, 78))
        if phrase == 1 and not last:
            parts[SNARE].append(note(bar, 15, 48))  # ghost before the next bar
        if phrase == 3 and not last:
            parts[SNARE].append(note(bar, 9, 42))
            parts[RIM].append(note(bar, 14, 96))

        open_hat = phrase in (1, 3) and not last
        for s in steps:
            if open_hat and s == 14:
                parts[OHAT].append(note(bar, s, 92))
            elif s % 2 == 0:
                parts[HAT].append(note(bar, s, 104 if s % 4 == 0 else 76))
            elif rng.random() < 0.22:
                parts[HAT].append(note(bar, s, 52))  # the odd swung sixteenth

        if full and not last:
            parts[RIDE] += [note(bar, s, 84 if s % 8 == 0 else 62) for s in (0, 4, 8, 12)]
            if phrase % 2 == 1:
                parts[CONGA] += [note(bar, 3, 70), note(bar, 11, 58)]
    return parts


def answer(bars):
    """An A minor pentatonic answer phrase on the internal FM keys."""
    A, C, D, E, G = 69, 72, 74, 76, 79
    phrase = [(0, A, 3), (3, C, 2), (6, E, 4), (10, D, 2), (12, C, 3)]
    reply = [(0, E, 3), (3, G, 2), (6, E, 2), (8, D, 2), (10, C, 2), (12, A, 4)]
    notes = []
    for bar in range(bars - 1):
        for step, pitch, length in phrase if bar % 2 == 0 else reply:
            notes.append(note(bar, step, 86, length * S16 - 20, pitch))
    return notes


def loops():
    """Each loop: (name, bars, {pad: notes})."""
    intro = {
        KEYS: on_the_one(0),
        CRACKLE: on_the_one(0),
        RIM: [note(b, s, 80) for b in range(4) for s in (4, 12)],
        HAT: [note(b, s, 70 if s % 4 else 96) for b in (2, 3) for s in range(0, 16, 2)],
        KICK: [note(3, 10, 104), note(3, 11, 96)],
        SNARE: [note(3, 13, 70), note(3, 14, 90), note(3, 15, 110)],
    }
    verse = beat(8, 87, full=False) | {KEYS: on_the_one(0), BASS: on_the_one(0), CRACKLE: on_the_one(0)}
    lift = beat(8, 88) | {
        KEYS: on_the_one(0), BASS: on_the_one(0), CRACKLE: on_the_one(0),
        VIBES: on_the_one(0), ANSWER: answer(8),
    }
    breakdown = {
        ZHENG: on_the_one(0),
        BASS: on_the_one(0),
        CRACKLE: on_the_one(0),
        KICK: [note(b, 0, 112) for b in range(4)] + [note(3, 10, 100), note(3, 11, 104)],
        RIM: [note(b, s, 86) for b in range(4) for s in (4, 12)],
        CONGA: [note(b, s, 64) for b in (1, 3) for s in (3, 11)],
        SNARE: [note(3, 12, 70), note(3, 13, 84), note(3, 14, 100), note(3, 15, 118)],
    }
    return [("Intro", 4, intro), ("Verse", 8, verse), ("Lift", 8, lift), ("Break", 4, breakdown)]


CHAIN = [0, 1, 2, 1, 2, 3]


def pad_cfg(**over):
    cfg = {
        "kind": "Audio", "vol": 0.8, "pan": 0.0, "mute": False, "solo": False, "choke": 0, "bus": 0,
        "pitch": 0.0, "mode": "OneShot", "start": 0, "end": 0, "attack": 0.0, "release": 0.02,
        "reverse": False, "cutoff": 1.0, "reso": 0.0, "lfo_rate": 2.0, "lfo_depth": 0.0,
        "lfo_target": "Off", "target": "Synth",
        "synth": {"engine": "Sub808", "octave": -2, "decay": 0.5, "tone": 0.5, "drive": 0.3},
        "human_time": 0.0, "human_vel": 0.0,
    }
    cfg.update(over)
    return cfg


def bus(level=1.0, slots=(), **over):
    empty = {"kind": "None", "params": [0.0, 0.0, 0.0], "on": True}
    filled = [{"kind": k, "params": p, "on": True} for k, p in slots]
    cfg = {"level": level, "mute": False, "out": 0, "slots": (filled + [empty] * 3)[:3],
           "sc_src": 0, "sc_amount": -0.7, "sc_release": 0.18}
    cfg.update(over)
    return cfg


def find(name):
    hits = sorted(SPLICE.rglob(name))
    if not hits:
        sys.exit(f"{name} is not in {SPLICE}")
    return hits[0]


def main():
    samples = OUT / "samples"
    samples.mkdir(parents=True, exist_ok=True)

    settings = {
        KICK: pad_cfg(bus=DRUMS, vol=0.9, human_vel=0.2),
        SNARE: pad_cfg(bus=DRUMS, vol=0.82, human_time=0.1, human_vel=0.25),
        HAT: pad_cfg(bus=DRUMS, vol=0.5, pan=0.15, choke=1, human_time=0.2, human_vel=0.5),
        OHAT: pad_cfg(bus=DRUMS, vol=0.5, pan=0.15, choke=1),
        RIM: pad_cfg(bus=DRUMS, vol=0.6, pan=-0.25),
        CLAP: pad_cfg(bus=DRUMS, vol=0.4, human_time=0.15),
        RIDE: pad_cfg(bus=DRUMS, vol=0.34, pan=-0.3, human_vel=0.4),
        CONGA: pad_cfg(bus=DRUMS, vol=0.5, pan=0.35, human_time=0.25, human_vel=0.4),
        KEYS: pad_cfg(bus=MUSIC, vol=0.72),
        BASS: pad_cfg(bus=LOW, vol=0.8),
        # The vibraphone pans slowly from side to side, a bar each way.
        VIBES: pad_cfg(bus=MUSIC, vol=0.55, lfo_target="Pan", lfo_depth=0.45, lfo_shape="Triangle", lfo_sync=4),
        ZHENG: pad_cfg(bus=MUSIC, vol=0.62, pan=0.1),
        CRACKLE: pad_cfg(bus=DUST, vol=0.7, attack=0.05),
        ANSWER: pad_cfg(kind="Midi", bus=MUSIC, vol=0.3, pan=0.2, human_time=0.3, human_vel=0.5,
                        synth={"engine": "Keys", "octave": 0, "decay": 0.45, "tone": 0.35, "drive": 0.1}),
    }
    names = {n: name for n, (name, _) in SOUNDS.items()} | {ANSWER: "FM Answer"}

    def pattern(notes, bars):
        return {"len_ticks": bars * BAR, "notes": sorted(notes, key=lambda n: n["start"])}

    scenes = []
    for name, bars, parts in loops():
        patterns = [[pad - 1, pattern(notes, bars)] for pad, notes in sorted(parts.items()) if notes]
        scenes.append({"name": name, "bars": bars, "patterns": patterns})
    first = dict(scenes[0]["patterns"])

    pads = []
    for number, cfg in settings.items():
        sample = None
        if number in SOUNDS:
            sample = f"A{number:02}.wav"
            shutil.copyfile(find(SOUNDS[number][1]), samples / sample)
        pads.append({
            "index": number - 1, "name": names[number], "cfg": cfg,
            "pattern": first.get(number - 1, pattern([], 2)),
            "transients": [], "sample": sample,
        })

    project = {
        "version": 1, "bpm": BPM, "swing": 0.56, "key": {"root": 9, "scale": "Minor"}, "banks": 4,
        "buses": [
            bus(0.9, [("Comp", [0.45, 0.5, 0.6]), ("Cassette", [0.3, 0.1, 0.75])]),          # drums
            bus(0.9, [("Vinyl", [0.25, 0.15, 0.35]), ("Cassette", [0.35, 0.25, 0.6])]),       # keys, vibes, strings
            bus(0.95, [("Cassette", [0.4, 0.05, 0.45])], sc_src=DRUMS, sc_amount=-0.4),       # bass ducks under the drums
            bus(0.55, sc_src=DRUMS, sc_amount=0.7, sc_release=0.35),                          # crackle swells with them
            bus(0.9, [("Comp", [0.6, 0.35, 0.5])]),                                           # master
        ],
        "conform_mode": "Strict", "auto_conform": False, "preserve_transients": True,
        "pads": pads, "fx_plugins": [],
        "scenes": scenes, "scene": 0, "chain": CHAIN, "chain_on": True,
    }
    (OUT / "project.json").write_text(json.dumps(project, indent=1))
    total = sum(scenes[i]["bars"] for i in CHAIN)
    print(f"wrote {OUT}: {len(pads)} pads, {len(scenes)} loops, a {total}-bar chain at {BPM:g} BPM")


main()
