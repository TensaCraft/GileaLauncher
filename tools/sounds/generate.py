"""Synthesises the launcher's extra click sounds into assets/sounds/ (stereo, 24-bit, 44.1 kHz).

Every sound is made here from sines, noise and envelopes — no recordings, nothing borrowed.
Run from the repository root: `python tools/sounds/generate.py` (needs numpy).
"""
import os
import wave

import numpy as np

RATE = 44100
OUT = os.path.join('assets', 'sounds')
# Loudness of every sound (RMS, dBFS) — the original three sit between -30 and -37 — and the
# most any peak may reach.
RMS_DB = -31.0
PEAK_LIMIT_DB = -3.0
rng = np.random.default_rng(20260929)


def t(seconds):
    return np.arange(int(RATE * seconds)) / RATE


def env(length, attack, decay):
    """A fast attack and an exponential decay (time constant `decay`), in seconds."""
    x = t(length)
    rise = np.clip(x / max(attack, 1e-4), 0, 1)
    return rise * np.exp(-x / decay)


def sweep(f0, f1, length, curve=1.0):
    """A sine whose pitch glides from f0 to f1 (exponentially when both are positive)."""
    x = t(length)
    k = (x / length) ** curve
    freq = f0 * (f1 / f0) ** k
    return np.sin(2 * np.pi * np.cumsum(freq) / RATE)


def band_noise(length, low, high):
    """White noise kept between `low` and `high` Hz."""
    n = int(RATE * length)
    spectrum = np.fft.rfft(rng.standard_normal(n))
    freqs = np.fft.rfftfreq(n, 1 / RATE)
    spectrum[(freqs < low) | (freqs > high)] = 0
    return np.fft.irfft(spectrum, n)


def pad(signal, total):
    out = np.zeros(int(RATE * total))
    out[: len(signal)] += signal[: len(out)]
    return out


def at(signal, offset, total):
    out = np.zeros(int(RATE * total))
    start = int(RATE * offset)
    end = min(len(out), start + len(signal))
    out[start:end] += signal[: end - start]
    return out


def finish(mono, width=0.0):
    """Match the loudness, fade the tail, and spread a little across the stereo field."""
    mono = mono - np.mean(mono)
    mono *= 10 ** (RMS_DB / 20) / np.sqrt(np.mean(mono ** 2))
    peak = np.max(np.abs(mono))
    if peak > 10 ** (PEAK_LIMIT_DB / 20):
        mono *= 10 ** (PEAK_LIMIT_DB / 20) / peak
    fade = int(RATE * 0.006)
    mono[-fade:] *= np.linspace(1, 0, fade)
    delay = int(RATE * 0.0004)
    late = np.concatenate([np.zeros(delay), mono[:-delay]])
    return np.stack([mono, (1 - width) * mono + width * late], axis=1)


def soft_pop():
    """A bubble popping: a quick downward chirp with a tiny click on top."""
    body = sweep(820, 260, 0.06, curve=0.6) * env(0.06, 0.001, 0.018)
    click = band_noise(0.004, 2000, 7000) * env(0.004, 0.0002, 0.0012) * 0.35
    return finish(pad(body, 0.14) + pad(click, 0.14), width=0.3)


def glass_tap():
    """A fingernail on a glass: bright inharmonic partials ringing out."""
    x = t(0.3)
    partials = [(2350, 1.0, 0.11), (3910, 0.55, 0.07), (5620, 0.3, 0.045), (7310, 0.15, 0.03)]
    ring = sum(a * np.sin(2 * np.pi * f * x) * np.exp(-x / d) for f, a, d in partials)
    ring *= np.clip(x / 0.0004, 0, 1)
    return finish(ring, width=0.5)


def water_drop():
    """A drop falling in water: a rising 'plip' and a softer echo of it."""
    plip = sweep(480, 1650, 0.05, curve=1.6) * env(0.05, 0.0015, 0.022)
    echo = sweep(620, 1500, 0.035, curve=1.6) * env(0.035, 0.002, 0.015) * 0.25
    return finish(pad(plip, 0.16) + at(echo, 0.055, 0.16), width=0.4)


def mechanical_key():
    """A clicky mechanical keyboard: the switch's click, the key bottoming out, its release."""
    click = band_noise(0.006, 2500, 9000) * env(0.006, 0.0002, 0.0015)
    x = t(0.06)
    thock = (np.sin(2 * np.pi * 190 * x) + 0.45 * np.sin(2 * np.pi * 410 * x)) * env(0.06, 0.0008, 0.012)
    release = band_noise(0.005, 3000, 8000) * env(0.005, 0.0002, 0.001) * 0.35
    total = 0.12
    return finish(pad(click, total) + at(thock * 0.9, 0.002, total) + at(release, 0.045, total), width=0.2)


def digital_blip():
    """A clean two-note interface blip, a fourth apart."""
    x = t(0.028)
    note = lambda f: np.sin(2 * np.pi * f * x) * np.sin(np.pi * x / x[-1]) ** 2
    return finish(pad(note(1319), 0.1) + at(note(1760) * 0.8, 0.03, 0.1), width=0.25)


def wood_tap():
    """A knock on a small wooden block: a short hollow tone over a filtered burst."""
    x = t(0.12)
    tone = (np.sin(2 * np.pi * 880 * x) + 0.5 * np.sin(2 * np.pi * 1450 * x)) * env(0.12, 0.0005, 0.022)
    knock = band_noise(0.012, 600, 2500) * env(0.012, 0.0003, 0.003) * 0.6
    return finish(pad(tone, 0.14) + pad(knock, 0.14), width=0.2)


def cosmic_zap():
    """A small sci-fi zap: a falling chirp with a shimmer of frequency modulation."""
    length = 0.09
    x = t(length)
    freq = 2400 * (500 / 2400) ** (x / length) + 180 * np.sin(2 * np.pi * 55 * x)
    zap = np.sin(2 * np.pi * np.cumsum(freq) / RATE) * env(length, 0.0008, 0.03)
    return finish(pad(zap, 0.13), width=0.6)


SOUNDS = {
    'soft-pop-click.wav': soft_pop,
    'glass-tap-click.wav': glass_tap,
    'water-drop-click.wav': water_drop,
    'mechanical-key-click.wav': mechanical_key,
    'digital-blip-click.wav': digital_blip,
    'wood-tap-click.wav': wood_tap,
    'cosmic-zap-click.wav': cosmic_zap,
}


def write(name, stereo):
    samples = np.clip(stereo, -1, 1) * (2 ** 23 - 1)
    ints = samples.astype(np.int32).reshape(-1)
    raw = b''.join(int(v).to_bytes(3, 'little', signed=True) for v in ints)
    with wave.open(os.path.join(OUT, name), 'wb') as w:
        w.setnchannels(2)
        w.setsampwidth(3)
        w.setframerate(RATE)
        w.writeframes(raw)


if __name__ == '__main__':
    for name, make in SOUNDS.items():
        write(name, make())
        print('wrote', name)
