"""Generate the plusplus README banner: black-purple gradient + film grain."""
import math
from PIL import Image, ImageChops, ImageDraw, ImageFont

W, H = 2048, 560
REPO = "/Users/weerachit/Documents/plusplus"

# ---- base gradient (computed small, upscaled) ----
gw, gh = 128, 35
base = Image.new("RGB", (gw, gh))
px = base.load()

TL = (6, 4, 10)      # near black, top-left
BR = (36, 20, 62)    # deep purple, bottom-right
GLOW = (107, 70, 193)  # purple glow accent

# glow center (relative) and radius
cx, cy, rad = 0.80, 1.10, 0.62

for y in range(gh):
    for x in range(gw):
        u, v = x / (gw - 1), y / (gh - 1)
        t = (u + v) / 2  # diagonal blend
        r = TL[0] + (BR[0] - TL[0]) * t
        g = TL[1] + (BR[1] - TL[1]) * t
        b = TL[2] + (BR[2] - TL[2]) * t
        # radial glow (aspect-corrected distance)
        d = math.hypot((u - cx) * (W / H), v - cy) / (rad * (W / H))
        glow = max(0.0, 1.0 - d) ** 2 * 0.45
        r += (GLOW[0] - r) * glow
        g += (GLOW[1] - g) * glow
        b += (GLOW[2] - b) * glow
        px[x, y] = (int(r), int(g), int(b))

base = base.resize((W, H), Image.BICUBIC)

# ---- film grain ----
noise = Image.effect_noise((W, H), 52).convert("RGB")
grained = ImageChops.soft_light(base, noise)
img = Image.blend(base, grained, 0.55)

# ---- logo, top-left ----
logo = Image.open(f"{REPO}/crates/app/assets/icon/png/icon-256.png").convert("RGBA")
logo = logo.resize((96, 96), Image.LANCZOS)
img.paste(logo, (88, 72), logo)

# ---- text, bottom-right (strix-style) ----
draw = ImageDraw.Draw(img)
TTC = "/System/Library/Fonts/HelveticaNeue.ttc"
brand_font = ImageFont.truetype(TTC, 40, index=0)   # Regular
head_font = ImageFont.truetype(TTC, 108, index=1)   # Bold

MARGIN = 96
brand = "plusplus"
headline = "Fast, native database GUI"

bw = draw.textlength(brand, font=brand_font)
draw.text((W - MARGIN - bw, 268), brand, font=brand_font, fill=(232, 228, 244))

hw = draw.textlength(headline, font=head_font)
draw.text((W - MARGIN - hw, 372), headline, font=head_font, fill=(250, 249, 253))

img.save(f"{REPO}/.github/readme-banner.jpg", quality=90, optimize=True)
print("saved", img.size)
