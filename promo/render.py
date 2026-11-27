"""Anuncio motion graphics de Chispa.

Dibuja cada cuadro con Pillow (a 2x y reducido, para bordes suaves), sintetiza el sonido con numpy
y arma el MP4 con ffmpeg. Todo el texto y las cifras salen de BEAT_SHEET.md.

    python render.py                 # render completo -> chispa-anuncio.mp4
    python render.py --frames 3.0 9.6 14.6   # styleframes PNG en ./frames
"""

import math
import os
import random
import subprocess
import sys
import wave
from multiprocessing import Pool

import numpy as np
from PIL import Image, ImageDraw, ImageFilter, ImageFont

HERE = os.path.dirname(os.path.abspath(__file__))
W, H, FPS, DUR = 1920, 1080, 30, 32.0
SS = 2  # supersampling
SW, SH = W * SS, H * SS

# ---------- marca ----------
BG = (17, 17, 19)
PANEL = (26, 26, 30)
PANEL2 = (34, 34, 40)
LINE = (52, 52, 60)
TEXT = (236, 236, 240)
MUTED = (139, 139, 150)
EMBER = (255, 122, 26)
EMBER_SOFT = (255, 160, 90)
OK = (61, 220, 132)
RED = (255, 59, 74)
TWITCH = (145, 70, 255)
KICK = (83, 252, 24)
YOUTUBE = (255, 48, 64)
OBS_BAR = (111, 134, 201)   # paleta validada (dataviz) contra #16161a
CHISPA_BAR = (226, 96, 12)

FONTS = r"C:\Windows\Fonts"
_font_cache = {}


def font(name, size):
    key = (name, size)
    if key not in _font_cache:
        _font_cache[key] = ImageFont.truetype(os.path.join(FONTS, name), int(size * SS))
    return _font_cache[key]


BLACK = "seguibl.ttf"
BOLD = "segoeuib.ttf"
SEMI = "seguisb.ttf"
REG = "segoeui.ttf"
MONO = "CascadiaMono.ttf"
SYM = "seguisym.ttf"

# ---------- easing ----------


def clamp(x, a=0.0, b=1.0):
    return max(a, min(b, x))


def prog(t, t0, dur):
    return clamp((t - t0) / dur) if dur > 0 else float(t >= t0)


def out_cubic(x):
    return 1 - (1 - x) ** 3


def in_out_cubic(x):
    return 4 * x ** 3 if x < 0.5 else 1 - (-2 * x + 2) ** 3 / 2


def out_back(x, s=1.4):
    x -= 1
    return 1 + (s + 1) * x ** 3 + s * x ** 2


def lerp(a, b, x):
    return a + (b - a) * x


def mix(c1, c2, x):
    return tuple(int(lerp(a, b, x)) for a, b in zip(c1, c2))


# ---------- capas ----------


class Layer:
    """Capa RGBA a escala 2x. Las coordenadas que recibe están en px de 1080p."""

    def __init__(self):
        self.im = Image.new("RGBA", (SW, SH), (0, 0, 0, 0))
        self.d = ImageDraw.Draw(self.im)

    @staticmethod
    def s(v):
        return int(round(v * SS))

    def rrect(self, x, y, w, h, r, fill=None, outline=None, width=2):
        self.d.rounded_rectangle(
            [self.s(x), self.s(y), self.s(x + w), self.s(y + h)], radius=self.s(r),
            fill=fill, outline=outline, width=self.s(width) if outline else 0)

    def rect(self, x, y, w, h, fill):
        self.d.rectangle([self.s(x), self.s(y), self.s(x + w), self.s(y + h)], fill=fill)

    def circle(self, cx, cy, r, fill=None, outline=None, width=2):
        self.d.ellipse([self.s(cx - r), self.s(cy - r), self.s(cx + r), self.s(cy + r)],
                       fill=fill, outline=outline, width=self.s(width) if outline else 0)

    def line(self, pts, fill, width=3):
        self.d.line([(self.s(x), self.s(y)) for x, y in pts], fill=fill, width=self.s(width), joint="curve")

    def poly(self, pts, fill):
        self.d.polygon([(self.s(x), self.s(y)) for x, y in pts], fill=fill)

    def text(self, x, y, s, fnt, fill, anchor="la"):
        self.d.text((self.s(x), self.s(y)), s, font=fnt, fill=fill, anchor=anchor)

    def tw(self, s, fnt):
        return fnt.getlength(s) / SS


def composite(base, layer, opacity=1.0):
    if opacity <= 0:
        return
    im = layer.im
    if opacity < 1:
        a = im.getchannel("A").point(lambda v: int(v * opacity))
        im = im.copy()
        im.putalpha(a)
    base.alpha_composite(im)


# ---------- fondo ----------

_bg_cache = {}


def background():
    if "bg" not in _bg_cache:
        im = Image.new("RGBA", (SW, SH), BG + (255,))
        d = ImageDraw.Draw(im)
        step = 44 * SS
        for yy in range(step // 2, SH, step):
            for xx in range(step // 2, SW, step):
                d.ellipse([xx - 2, yy - 2, xx + 2, yy + 2], fill=(255, 255, 255, 12))
        _bg_cache["bg"] = im
        # brillo radial (más grande que el cuadro para poder moverlo)
        gw, gh = SW * 2, SH * 2
        yy, xx = np.mgrid[0:gh, 0:gw].astype(np.float32)
        r = np.sqrt(((xx - gw / 2) / (SW * 0.42)) ** 2 + ((yy - gh / 2) / (SH * 0.55)) ** 2)
        a = np.clip(1 - r, 0, 1) ** 2 * 70
        g = np.zeros((gh, gw, 4), np.uint8)
        g[..., 0], g[..., 1], g[..., 2] = EMBER
        g[..., 3] = a.astype(np.uint8)
        _bg_cache["glow"] = Image.fromarray(g, "RGBA")
    return _bg_cache["bg"], _bg_cache["glow"]


# centro del brillo por escena (px 1080p)
GLOW_KEYS = [(0.0, 1500, 700), (3.2, 960, 540), (6.6, 1400, 560), (11.6, 520, 520),
             (16.6, 960, 560), (21.6, 1500, 620), (27.0, 960, 480)]


def glow_center(t):
    for (t0, x0, y0), (t1, x1, y1) in zip(GLOW_KEYS, GLOW_KEYS[1:]):
        if t < t1:
            k = in_out_cubic(prog(t, t1 - 0.9, 0.9))
            return lerp(x0, x1, k), lerp(y0, y1, k)
    return GLOW_KEYS[-1][1:]


# ---------- piezas reutilizables ----------


def bolt_points(cx, cy, size):
    # Rayo de la marca (path 24x24 del ícono de Chispa)
    pts = [(13, 2), (4, 14), (11, 14), (10, 22), (19, 10), (12, 10)]
    k = size / 24
    return [(cx + (x - 12) * k, cy + (y - 12) * k) for x, y in pts]


def kicker(L, x, y, s, op=1.0, color=EMBER):
    L.text(x, y, s, font(BOLD, 22), color + (int(255 * op),))


def headline(L, x, y, s, size=76, color=TEXT):
    L.text(x, y, s, font(BLACK, size), color + (255,))


def exit_k(t, end):
    """0 normal → 1 salido. La salida dura 0,35 s y termina en `end`."""
    return in_out_cubic(prog(t, end - 0.35, 0.35))


def bezier(p0, p1, p2, p3, n=48, upto=1.0):
    pts = []
    for i in range(int(n * upto) + 1):
        u = i / n
        a = (1 - u) ** 3
        b = 3 * (1 - u) ** 2 * u
        c = 3 * (1 - u) * u ** 2
        d = u ** 3
        pts.append((a * p0[0] + b * p1[0] + c * p2[0] + d * p3[0], a * p0[1] + b * p1[1] + c * p2[1] + d * p3[1]))
    return pts


# ---------- escenas ----------


def scene_hook(base, t):
    end = 3.2
    ex = exit_k(t, end)
    L = Layer()
    stutters = [1.15, 1.85, 2.4]
    jit = 0
    for s in stutters:
        if s <= t < s + 0.1:
            jit = 10
    words1 = ["¿Tu", "PC", "se", "traba"]
    words2 = ["cuando", "transmitís?"]
    f = font(BLACK, 112)
    y0 = 330 - ex * 60
    x = 180
    for i, w in enumerate(words1 + words2):
        line2 = i >= len(words1)
        if i == len(words1):
            x = 180
        p = out_cubic(prog(t, 0.15 + i * 0.13, 0.45))
        if p <= 0:
            continue
        yy = y0 + (140 if line2 else 0) + (1 - p) * 40
        col = EMBER if w == "traba" else TEXT
        dx = jit if int(t * FPS) % 2 == 0 else -jit
        if jit and w == "traba":
            L.text(x + dx + 8, yy, w, f, RED + (140,))
        L.text(x + dx, yy, w, f, col + (int(255 * p),))
        x += L.tw(w + " ", f)
    # tira de cuadros que avanza a los saltos (demostración abstracta de lag)
    strip_y = 690 - ex * 60
    p = out_cubic(prog(t, 0.5, 0.5))
    # posición: avanza parejo pero se congela en cada stutter
    frozen = sum(min(max(t - s, 0), 0.32) for s in stutters)
    pos = (t - frozen) * 220
    for i in range(-1, 14):
        xx = 180 + i * 130 - (pos % 130)
        lag = any(s <= t < s + 0.32 for s in stutters) and i in (5, 6)
        edge = clamp(min(xx - 180, 1740 - (xx + 112)) / 160)
        q = p * edge
        if q <= 0:
            continue
        fill = (60, 30, 34, int(255 * q)) if lag else PANEL + (int(255 * q),)
        out = RED + (int(220 * q),) if lag else LINE + (int(255 * q),)
        L.rrect(xx, strip_y, 112, 70, 10, fill=fill, outline=out, width=2)
    if any(s <= t < s + 0.32 for s in stutters):
        L.text(180, strip_y + 96, "frames perdidos", font(SEMI, 24), RED + (int(230 * p),))
    composite(base, L, 1 - ex)


def scene_logo(base, t):
    start, end = 3.2, 6.6
    u = t - start
    ex = exit_k(t, end)
    L = Layer()
    cx, cy = 690, 500 - ex * 60
    p = prog(u, 0.0, 0.7)
    sc = out_back(p) if p < 1 else 1
    tile = 230 * (0.7 + 0.3 * sc)
    L.rrect(cx - tile / 2, cy - tile / 2, tile, tile, 54 * tile / 230, fill=PANEL + (255,), outline=LINE + (255,), width=2)
    bp = out_back(prog(u, 0.15, 0.6))
    if bp > 0:
        L.poly(bolt_points(cx, cy, 170 * bp), EMBER + (255,))
    # chispitas que salen del rayo
    sp = prog(u, 0.25, 0.9)
    if 0 < sp < 1:
        rnd = random.Random(7)
        for k in range(9):
            ang = rnd.uniform(0, 2 * math.pi)
            dist = 90 + 160 * out_cubic(sp) * rnd.uniform(0.6, 1.1)
            r = 6 * (1 - sp) + 1
            L.circle(cx + math.cos(ang) * dist, cy + math.sin(ang) * dist, r, fill=EMBER_SOFT + (int(255 * (1 - sp)),))
    # palabra con revelado por máscara desde el rayo
    wp = out_cubic(prog(u, 0.45, 0.7))
    if wp > 0:
        T = Layer()
        T.text(cx + 170, cy - 92, "Chispa", font(BLACK, 168), TEXT + (255,))
        mask_w = int(SS * (cx + 170 + 760 * wp))
        T.im.paste((0, 0, 0, 0), (mask_w, 0, SW, SH))
        L.im.alpha_composite(T.im)
    tp = out_cubic(prog(u, 1.15, 0.6))
    if tp > 0:
        L.text(cx + 176, cy + 146 + (1 - tp) * 20, "Grabá, clipeá y transmití en PCs flojas.", font(SEMI, 40), (201, 201, 210, int(255 * tp)))
    composite(base, L, 1 - ex)


def icon_cpu(L, x, y, s, col):
    L.rrect(x + s * .2, y + s * .2, s * .6, s * .6, s * .08, outline=col, width=3)
    L.rrect(x + s * .36, y + s * .36, s * .28, s * .28, s * .04, fill=col)
    for k in range(3):
        o = s * (.32 + k * .18)
        L.line([(x + o, y + s * .06), (x + o, y + s * .2)], col, 3)
        L.line([(x + o, y + s * .8), (x + o, y + s * .94)], col, 3)
        L.line([(x + s * .06, y + o), (x + s * .2, y + o)], col, 3)
        L.line([(x + s * .8, y + o), (x + s * .94, y + o)], col, 3)


def icon_ram(L, x, y, s, col):
    L.rrect(x + s * .05, y + s * .3, s * .9, s * .36, s * .05, outline=col, width=3)
    for k in range(4):
        L.rect(x + s * (.15 + k * .19), y + s * .38, s * .12, s * .16, col)
    for k in range(6):
        L.line([(x + s * (.14 + k * .14), y + s * .66), (x + s * (.14 + k * .14), y + s * .76)], col, 3)


def icon_gpu(L, x, y, s, col):
    L.rrect(x + s * .05, y + s * .25, s * .9, s * .48, s * .06, outline=col, width=3)
    L.circle(x + s * .35, y + s * .49, s * .15, outline=col, width=3)
    L.circle(x + s * .35, y + s * .49, s * .04, fill=col)
    L.rect(x + s * .62, y + s * .38, s * .22, s * .06, col)
    L.rect(x + s * .62, y + s * .52, s * .22, s * .06, col)
    L.line([(x + s * .15, y + s * .73), (x + s * .15, y + s * .85)], col, 3)


def scene_autoconfig(base, t):
    start, end = 6.6, 11.6
    u = t - start
    ex = exit_k(t, end)
    dy = -ex * 60
    L = Layer()
    hp = out_cubic(prog(u, 0.0, 0.5))
    kicker(L, 180, 150 + dy + (1 - hp) * 20, "AUTOCONFIGURACIÓN", hp)
    if hp > 0:
        L.text(180, 180 + dy + (1 - hp) * 24, "Se configura sola", font(BLACK, 80), TEXT + (int(255 * hp),))
    chips = [("CPU", "4 hilos", icon_cpu), ("RAM", "8 GB", icon_ram), ("GPU", "RX 560", icon_gpu)]
    cy0 = 360
    merge = (1010, 560 + dy)
    for i, (lab, val, ic) in enumerate(chips):
        p = out_cubic(prog(u, 0.7 + i * 0.4, 0.45))
        if p <= 0:
            continue
        x, y = 180 + (1 - p) * -40, cy0 + i * 150 + dy
        a = int(255 * p)
        L.rrect(x, y, 520, 118, 18, fill=PANEL + (a,), outline=LINE + (a,), width=2)
        ic(L, x + 26, y + 24, 70, EMBER + (a,))
        L.text(x + 122, y + 26, lab, font(SEMI, 22), MUTED + (a,))
        L.text(x + 122, y + 52, val, font(BOLD, 44), TEXT + (a,))
        # barrido de lectura (detectando)
        sp = prog(u, 0.7 + i * 0.4, 0.55)
        if 0 < sp < 1:
            sx = x + 520 * sp
            L.rect(sx - 2, y + 8, 4, 102, EMBER_SOFT + (int(200 * (1 - sp)),))
        cp = prog(u, 0.7 + i * 0.4 + 0.5, 0.25)
        if cp > 0:
            L.circle(x + 486, y + 30, 11, fill=OK + (int(255 * cp),))
            L.line([(x + 480, y + 30), (x + 485, y + 35), (x + 492, y + 25)], BG + (int(255 * cp),), 3)
        # conector hacia el resultado
        lp = in_out_cubic(prog(u, 1.8 + i * 0.12, 0.7))
        if lp > 0:
            p0 = (x + 520, y + 59)
            pts = bezier(p0, (p0[0] + 180, p0[1]), (merge[0] - 180, merge[1]), merge, upto=lp)
            L.line(pts, EMBER + (int(200 * (1 - 0.4 * prog(u, 3.0, 0.6))),), 3)
    fp = prog(u, 1.4, 0.5)
    if fp > 0:
        L.text(180, 820 + dy, "PC de prueba real · Chispa detecta esto sola al abrirse", font(REG, 22), MUTED + (int(255 * fp),))
    # tarjeta resultado
    rp = out_back(prog(u, 2.8, 0.55), 1.1)
    if rp > 0:
        a = int(255 * clamp(rp))
        x, y = 1080, 380 + dy + (1 - clamp(rp)) * 30
        L.line([merge, (x, merge[1])], EMBER + (a,), 3)
        L.rrect(x, y, 640, 360, 26, fill=(40, 24, 14, a), outline=EMBER + (a,), width=3)
        L.text(x + 40, y + 36, "PERFIL ELEGIDO", font(BOLD, 22), EMBER + (a,))
        L.text(x + 40, y + 70, "Potato", font(BLACK, 104), TEXT + (a,))
        L.text(x + 40, y + 214, "720p · 30 fps · 3500 kbps", font(SEMI, 34), (230, 205, 185, a))
        L.text(x + 40, y + 266, "Encoder por GPU (AMF)", font(SEMI, 34), (230, 205, 185, a))
    composite(base, L, 1 - ex)


def icon_mic(L, cx, cy, s, col):
    L.rrect(cx - s * .16, cy - s * .42, s * .32, s * .52, s * .16, fill=col)
    L.d.arc([L.s(cx - s * .3), L.s(cy - s * .2), L.s(cx + s * .3), L.s(cy + s * .32)], 0, 180, fill=col, width=L.s(4))
    L.line([(cx, cy + s * .32), (cx, cy + s * .44)], col, 4)
    L.line([(cx - s * .16, cy + s * .44), (cx + s * .16, cy + s * .44)], col, 4)


def scene_voice(base, t):
    start, end = 11.6, 16.6
    u = t - start
    ex = exit_k(t, end)
    dy = -ex * 60
    L = Layer()
    hp = out_cubic(prog(u, 0.0, 0.5))
    kicker(L, 180, 150 + dy + (1 - hp) * 20, "COMANDOS DE VOZ", hp)
    if hp > 0:
        L.text(180, 180 + dy + (1 - hp) * 24, "Manejala con la voz", font(BLACK, 80), TEXT + (int(255 * hp),))
    # micrófono escuchando
    mp = out_back(prog(u, 0.3, 0.5))
    mcx, mcy = 290, 520 + dy
    result_t = 2.8
    listening = 0.5 < u < result_t
    if mp > 0:
        a = int(255 * clamp(mp))
        if listening:
            for k in range(2):
                ph = ((u * 1.3 + k * 0.5) % 1)
                L.circle(mcx, mcy, 70 + 60 * ph, outline=EMBER + (int(160 * (1 - ph)),), width=3)
        L.circle(mcx, mcy, 70 * clamp(mp), fill=EMBER + (a,))
        icon_mic(L, mcx, mcy + 4, 80 * clamp(mp), BG + (a,))
    # burbuja con la frase
    phrase = "«Chispa, clipeá eso»"
    bp = out_cubic(prog(u, 0.55, 0.4))
    dim = 0.55 if u > result_t + 0.2 else 1.0
    if bp > 0:
        a = int(255 * bp * dim)
        bx, by = 420, 450 + dy
        L.rrect(bx, by, 780, 140, 30, fill=PANEL2 + (a,), outline=LINE + (a,), width=2)
        L.poly([(bx + 2, by + 56), (bx - 26, by + 80), (bx + 2, by + 92)], PANEL2 + (a,))
        n = int(len(phrase) * prog(u, 0.7, 1.5))
        L.text(bx + 40, by + 34, phrase[:n], font(BOLD, 58), TEXT + (a,))
        if 0.7 < u < 2.3 and int(u * 4) % 2 == 0:
            cx = bx + 40 + L.tw(phrase[:n], font(BOLD, 58)) + 6
            L.rect(cx, by + 44, 4, 60, EMBER + (a,))
        # forma de onda
        for k in range(36):
            amp = 0.15
            if 0.7 < u < 2.3:
                amp = 0.25 + 0.75 * abs(math.sin(u * 9 + k * 0.7) * math.sin(u * 3.1 + k * 0.33))
            h = 8 + 46 * amp
            L.rrect(bx + 40 + k * 20, by + 170 - h / 2 + 24, 8, h, 4, fill=EMBER + (int(a * (0.9 if listening else 0.35)),))
    # resultado: clip guardado
    rp = out_back(prog(u, result_t, 0.5), 1.2)
    if rp > 0:
        a = int(255 * clamp(rp))
        rx, ry = 1280, 430 + dy + (1 - clamp(rp)) * 30
        lp = in_out_cubic(prog(u, result_t - 0.35, 0.35))
        L.line([(1200, 520 + dy), (1200 + 80 * lp, 520 + dy)], EMBER + (220,), 3)
        L.rrect(rx, ry, 460, 180, 24, fill=(40, 24, 14, a), outline=EMBER + (a,), width=3)
        L.text(rx + 34, ry + 30, "✂", font(SYM, 54), EMBER + (a,))
        L.text(rx + 104, ry + 34, "Clip guardado", font(BLACK, 46), TEXT + (a,))
        L.text(rx + 106, ry + 102, "últimos 30 s", font(SEMI, 32), (230, 205, 185, a))
    kp = out_cubic(prog(u, result_t + 0.7, 0.45))
    if kp > 0:
        a = int(255 * kp)
        kx, ky = 420, 760 + dy + (1 - kp) * 20
        L.text(kx, ky + 12, "o con", font(SEMI, 36), MUTED + (a,))
        L.rrect(kx + 110, ky, 200, 70, 14, fill=PANEL + (a,), outline=MUTED + (a,), width=2)
        L.rrect(kx + 110, ky + 60, 200, 10, 6, fill=LINE + (a,))
        L.text(kx + 210, ky + 32, "Alt+F10", font(MONO, 34), TEXT + (a,), anchor="mm")
        L.text(kx + 340, ky + 12, "sin salir del juego", font(SEMI, 36), MUTED + (a,))
    composite(base, L, 1 - ex)


def scene_multistream(base, t):
    start, end = 16.6, 21.6
    u = t - start
    ex = exit_k(t, end)
    dy = -ex * 60
    L = Layer()
    hp = out_cubic(prog(u, 0.0, 0.5))
    kicker(L, 180, 150 + dy + (1 - hp) * 20, "MULTISTREAM", hp)
    if hp > 0:
        L.text(180, 180 + dy + (1 - hp) * 24, "Tres plataformas.", font(BLACK, 80), TEXT + (int(255 * hp),))
    h2 = out_cubic(prog(u, 0.35, 0.5))
    if h2 > 0:
        x2 = 180 + L.tw("Tres plataformas. ", font(BLACK, 80))
        L.text(x2, 180 + dy + (1 - h2) * 24, "La CPU de una.", font(BLACK, 80), EMBER + (int(255 * h2),))
    # nodo fuente
    np_ = out_cubic(prog(u, 0.5, 0.45))
    nx, ny = 180, 480 + dy
    if np_ > 0:
        a = int(255 * np_)
        L.rrect(nx, ny, 420, 220, 26, fill=(40, 24, 14, a), outline=EMBER + (a,), width=3)
        L.text(nx + 40, ny + 36, "1 encode", font(BLACK, 72), EMBER_SOFT + (a,))
        L.text(nx + 42, ny + 136, "H.264 por GPU + AAC", font(SEMI, 32), (230, 205, 185, a))
    dests = [("Twitch", TWITCH, "RTMP"), ("Kick", KICK, "RTMPS"), ("YouTube", YOUTUBE, "RTMP")]
    for i, (name, col, proto) in enumerate(dests):
        dx, dyy = 1240, 380 + i * 160 + dy
        cp = out_cubic(prog(u, 0.8 + i * 0.15, 0.4))
        lp = in_out_cubic(prog(u, 1.0 + i * 0.3, 0.45))
        arrived = lp >= 1
        p0 = (nx + 420, ny + 110)
        p3 = (dx, dyy + 56)
        if lp > 0:
            pts = bezier(p0, (p0[0] + 260, p0[1]), (p3[0] - 260, p3[1]), p3, upto=lp)
            L.line(pts, LINE + (255,), 4)
            L.line(pts, EMBER + (200,), 2)
        # paquetes viajando (flujo continuo una vez conectado)
        if arrived:
            full = bezier(p0, (p0[0] + 260, p0[1]), (p3[0] - 260, p3[1]), p3, n=60)
            for k in range(3):
                ph = ((u * 0.9 + k / 3 + i * 0.13) % 1)
                px, py = full[int(ph * 60)]
                L.circle(px, py, 6, fill=EMBER_SOFT + (230,))
        if cp > 0:
            a = int(255 * cp)
            glow = prog(u, 1.45 + i * 0.3, 0.2) * (1 - prog(u, 1.8 + i * 0.3, 0.6))
            border = mix(LINE, col, 0.35 + 0.65 * clamp(prog(u, 1.45 + i * 0.3, 0.2)))
            L.rrect(dx, dyy, 440, 112, 20, fill=PANEL + (a,), outline=border + (a,), width=3)
            if glow > 0:
                L.rrect(dx - 6, dyy - 6, 452, 124, 24, outline=col + (int(160 * glow),), width=3)
            L.rrect(dx + 28, dyy + 34, 44, 44, 10, fill=col + (a,))
            L.text(dx + 96, dyy + 26, name, font(BOLD, 50), TEXT + (a,))
            if arrived:
                L.text(dx + 410, dyy + 56, proto, font(MONO, 22), MUTED + (a,), anchor="rm")
    fp = prog(u, 2.3, 0.5)
    if fp > 0:
        L.text(180, 880 + dy, "Se codifica una sola vez: solo se multiplica el uso de red.", font(SEMI, 30), MUTED + (int(255 * fp),))
    composite(base, L, 1 - ex)


def scene_bench(base, t):
    start, end = 21.6, 27.0
    u = t - start
    ex = exit_k(t, end)
    dy = -ex * 60
    L = Layer()
    hp = out_cubic(prog(u, 0.0, 0.5))
    kicker(L, 180, 150 + dy + (1 - hp) * 20, "MEDIDO EN UNA PC FLOJA", hp)
    if hp > 0:
        L.text(180, 180 + dy + (1 - hp) * 24, "Más liviana que OBS", font(BLACK, 80), TEXT + (int(255 * hp),))
    scale = 1060 / 180  # px por MB
    bars = [("OBS 32.0.4", 166, OBS_BAR, 0.6), ("Chispa", 92, CHISPA_BAR, 1.0)]
    for i, (name, mb, col, t0) in enumerate(bars):
        y = 420 + i * 190 + dy
        lp = out_cubic(prog(u, t0 - 0.3, 0.4))
        if lp <= 0:
            continue
        a = int(255 * lp)
        L.text(180, y, name, font(BOLD if i else SEMI, 36), (TEXT if i else MUTED) + (a,))
        g = out_cubic(prog(u, t0, 1.1))
        if g > 0:
            L.rrect(180, y + 56, max(16, mb * scale * g), 70, 8, fill=col + (255,))
            vp = out_cubic(prog(u, t0 + 0.95, 0.3))
            if vp > 0:
                L.text(180 + mb * scale + 22 + (1 - vp) * 12, y + 91, f"{mb} MB", font(BLACK, 44), TEXT + (int(255 * vp),), anchor="lm")
    bp = out_back(prog(u, 2.4, 0.5), 1.6)
    if bp > 0:
        a = int(255 * clamp(bp))
        bx, by = 1420, 560 + dy
        r = 150 * clamp(bp, 0, 1.2)
        L.circle(bx + 150, by + 60, r, fill=(40, 24, 14, a), outline=EMBER + (a,), width=4)
        L.text(bx + 150, by + 40, "−45 %", font(BLACK, 76), EMBER_SOFT + (a,), anchor="mm")
        L.text(bx + 150, by + 104, "de RAM", font(BOLD, 34), TEXT + (a,), anchor="mm")
    fp = prog(u, 1.6, 0.5)
    if fp > 0:
        L.text(180, 860 + dy, "RAM promedio · misma escena 1080p→720p30, encoder AMF · RX 560, 4 hilos, 8 GB",
               font(REG, 24), MUTED + (int(255 * fp),))
    composite(base, L, 1 - ex)


def scene_cta(base, t):
    start = 27.0
    u = t - start
    L = Layer()
    cx = 960
    p = out_back(prog(u, 0.1, 0.6))
    tile = 150 * clamp(p, 0, 1.1)
    if p > 0:
        L.rrect(cx - tile / 2, 250 - tile / 2, tile, tile, 36 * tile / 150, fill=PANEL + (255,), outline=LINE + (255,), width=2)
        L.poly(bolt_points(cx, 250, 110 * clamp(p, 0, 1.1)), EMBER + (255,))
    sp = prog(u, 0.5, 1.0)
    if 0 < sp < 1:
        rnd = random.Random(3)
        for k in range(10):
            ang = rnd.uniform(0, 2 * math.pi)
            dist = 70 + 140 * out_cubic(sp) * rnd.uniform(0.6, 1.1)
            L.circle(cx + math.cos(ang) * dist, 250 + math.sin(ang) * dist, 5 * (1 - sp) + 1, fill=EMBER_SOFT + (int(255 * (1 - sp)),))
    wp = out_cubic(prog(u, 0.5, 0.5))
    if wp > 0:
        L.text(cx, 400 + (1 - wp) * 20, "Chispa", font(BLACK, 120), TEXT + (int(255 * wp),), anchor="mt")
    gp = out_cubic(prog(u, 0.9, 0.5))
    if gp > 0:
        L.text(cx, 560 + (1 - gp) * 20, "Gratis y open source", font(BOLD, 56), EMBER_SOFT + (int(255 * gp),), anchor="mt")
    up = out_cubic(prog(u, 1.3, 0.5))
    if up > 0:
        a = int(255 * up)
        url = "github.com/juanipro1979/chispa"
        f = font(MONO, 42)
        w = L.tw(url, f) + 80
        L.rrect(cx - w / 2, 680 + (1 - up) * 20, w, 86, 43, fill=PANEL + (a,), outline=EMBER + (a,), width=3)
        L.text(cx, 723 + (1 - up) * 20, url, f, TEXT + (a,), anchor="mm")
    mp = prog(u, 1.8, 0.5)
    if mp > 0:
        L.text(cx, 820, "Windows 10/11 · licencia GPL-3.0", font(SEMI, 30), MUTED + (int(255 * mp),), anchor="mt")
    composite(base, L)


SCENES = [(0.0, 3.2, scene_hook), (3.2, 6.6, scene_logo), (6.6, 11.6, scene_autoconfig),
          (11.6, 16.6, scene_voice), (16.6, 21.6, scene_multistream), (21.6, 27.0, scene_bench),
          (27.0, DUR + 1, scene_cta)]


def render_frame(i):
    t = i / FPS
    bg, glow = background()
    base = bg.copy()
    gx, gy = glow_center(t)
    gw, gh = glow.size
    left = int(gw / 2 - gx * SS)
    top = int(gh / 2 - gy * SS)
    base.alpha_composite(glow.crop((left, top, left + SW, top + SH)))
    for t0, t1, fn in SCENES:
        if t0 <= t < t1:
            fn(base, t)
    # barra de progreso fina de la marca (microdetalle) y fundido de entrada
    out = base.convert("RGB").resize((W, H), Image.LANCZOS)
    arr = np.asarray(out).astype(np.int16)
    noise = np.random.default_rng(i).integers(-1, 2, size=(H, W, 1), dtype=np.int16)
    out = Image.fromarray(np.clip(arr + noise, 0, 255).astype(np.uint8), "RGB")
    if t < 0.25:
        out = Image.blend(Image.new("RGB", (W, H), BG), out, t / 0.25)
    return out.tobytes()


# ---------- sonido ----------

SR = 48000


def synth_audio(path):
    n = int(SR * DUR)
    out = np.zeros(n, np.float32)
    rng = np.random.default_rng(1)
    tt = np.arange(n) / SR

    def add(sig, at, gain):
        s = int(at * SR)
        e = min(n, s + len(sig))
        out[s:e] += sig[: e - s] * gain

    def env(length, attack=0.005, decay=0.2):
        x = np.arange(int(length * SR)) / SR
        return np.minimum(x / attack, 1) * np.exp(-x / decay)

    # pad grave: acorde La menor suave con trémolo lento
    pad = np.zeros(n, np.float32)
    for f, g in [(110, 1), (164.81, .6), (220, .5), (261.63, .35)]:
        pad += g * np.sin(2 * np.pi * f * tt) * (0.8 + 0.2 * np.sin(2 * np.pi * 0.2 * tt))
    fade = np.minimum(tt / 2.0, 1) * np.minimum((DUR - tt) / 1.5, 1)
    out += pad * fade * 0.018

    def whoosh(length=0.45):
        m = int(length * SR)
        noise = rng.standard_normal(m).astype(np.float32)
        # filtro paso bajo con corte que sube y baja (barrido)
        y = np.zeros(m, np.float32)
        acc = 0.0
        for k in range(m):
            x = k / m
            a = 0.02 + 0.35 * math.sin(math.pi * x)
            acc += a * (noise[k] - acc)
            y[k] = acc
        return y * np.sin(np.pi * np.linspace(0, 1, m)) ** 2

    def tick(freq=2400, length=0.03):
        x = np.arange(int(length * SR)) / SR
        return np.sin(2 * np.pi * freq * x) * np.exp(-x / 0.006)

    def chime(base_f=880, length=1.4):
        x = np.arange(int(length * SR)) / SR
        s = sum(g * np.sin(2 * np.pi * base_f * h * x) * np.exp(-x / (0.5 / h)) for h, g in [(1, 1), (2.01, .5), (3.02, .25), (4.2, .12)])
        return s * np.minimum(x / 0.003, 1)

    def blip(length=0.07):
        x = np.arange(int(length * SR)) / SR
        sq = np.sign(np.sin(2 * np.pi * 140 * x))
        return sq * np.exp(-x / 0.02)

    def shutter():
        a = rng.standard_normal(int(0.05 * SR)).astype(np.float32) * env(0.05, 0.001, 0.008)
        b = rng.standard_normal(int(0.06 * SR)).astype(np.float32) * env(0.06, 0.001, 0.012)
        s = np.zeros(int(0.12 * SR), np.float32)
        s[: len(a)] += a
        s[int(0.055 * SR): int(0.055 * SR) + len(b)] += b * 0.8
        return s

    def ping(f):
        x = np.arange(int(0.35 * SR)) / SR
        return np.sin(2 * np.pi * f * x) * np.exp(-x / 0.08) * np.minimum(x / 0.002, 1)

    def chord(fs, length=0.9):
        x = np.arange(int(length * SR)) / SR
        return sum(np.sin(2 * np.pi * f * x) for f in fs) * np.exp(-x / 0.3) * np.minimum(x / 0.01, 1) / len(fs)

    for s in [1.15, 1.85, 2.4]:
        add(blip(), s, 0.22)
    add(chime(880), 3.4, 0.16)
    add(chime(1320), 3.55, 0.07)
    for at in [6.25, 11.25, 16.25, 21.25, 26.65]:
        add(whoosh(), at, 0.5)
    for at in [7.3, 7.7, 8.1]:
        add(tick(2200), at, 0.25)
    add(chord([523.25, 659.25, 783.99]), 9.4, 0.2)
    for k in range(18):
        add(tick(3200 + (k % 3) * 300, 0.02), 12.3 + k * 1.5 / 18, 0.08)
    add(shutter(), 14.4, 0.35)
    for i, f in enumerate([660, 880, 1100]):
        add(ping(f), 17.6 + i * 0.3 + 0.45, 0.16)
    add(chord([392, 523.25, 659.25, 1046.5]), 24.0, 0.22)
    add(chime(1046.5), 27.15, 0.15)
    add(chime(1568), 27.3, 0.06)

    # limitador suave: sube la sonoridad sin recortar los ataques
    out = out / np.max(np.abs(out))
    drive = float(os.environ.get("DRIVE", "1.6"))
    out = np.tanh(out * drive) / np.tanh(drive)
    out = out * 10 ** (-1.5 / 20)  # pico −1,5 dBFS
    st = np.stack([out, out], 1)
    pcm = (st * 32767).astype("<i2")
    with wave.open(path, "wb") as w:
        w.setnchannels(2)
        w.setsampwidth(2)
        w.setframerate(SR)
        w.writeframes(pcm.tobytes())
    rms = 20 * np.log10(np.sqrt(np.mean(out ** 2)) + 1e-9)
    return rms


def main():
    if "--frames" in sys.argv:
        os.makedirs(os.path.join(HERE, "frames"), exist_ok=True)
        for s in sys.argv[sys.argv.index("--frames") + 1:]:
            t = float(s)
            img = Image.frombytes("RGB", (W, H), render_frame(int(round(t * FPS))))
            p = os.path.join(HERE, "frames", f"frame_{t:05.2f}.png")
            img.save(p)
            print(p)
        return
    wav = os.path.join(HERE, "audio.wav")
    rms = synth_audio(wav)
    print(f"audio RMS {rms:.1f} dBFS, pico -1.5 dBFS")
    out = os.path.join(HERE, "chispa-anuncio.mp4")
    cmd = ["ffmpeg", "-y", "-loglevel", "error",
           "-f", "rawvideo", "-pix_fmt", "rgb24", "-s", f"{W}x{H}", "-r", str(FPS), "-i", "-",
           "-i", wav,
           "-vf", "scale=out_color_matrix=bt709:out_range=tv",
           "-c:v", "libx264", "-preset", "slow", "-crf", "18", "-pix_fmt", "yuv420p",
           "-x264opts", "colorprim=bt709:transfer=bt709:colormatrix=bt709",
           "-c:a", "aac", "-strict", "-2", "-b:a", "192k", "-movflags", "+faststart", "-shortest", out]
    ff = subprocess.Popen(cmd, stdin=subprocess.PIPE)
    total = int(DUR * FPS)
    workers = int(os.environ.get("WORKERS", "2"))
    with Pool(workers) as pool:
        for k, buf in enumerate(pool.imap(render_frame, range(total), chunksize=4)):
            ff.stdin.write(buf)
            if k % 60 == 0:
                print(f"{k}/{total}", flush=True)
    ff.stdin.close()
    ff.wait()
    print("listo:", out, ff.returncode)


if __name__ == "__main__":
    main()
