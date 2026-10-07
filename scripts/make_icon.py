r"""產生 Workspace Hub 的應用程式圖示（Tauri 所需全部尺寸）。

主題：workspace 的統一入口／儀表板 —— 深藍圓角底、2×2 圓角方格，左上一格以琥珀色「亮起」。
無文字、無漸層；小尺寸（16/32px）靠粗間距與單一亮色仍可辨識。

用法（repo 根或任何目錄皆可）：
    E:\Kyle\Workspace\agent-harness\runtime\.venv\Scripts\python.exe scripts\make_icon.py [--out <dir>]
預設輸出到 src-tauri/icons/（覆蓋），改完要重跑 scripts/build-agent-desktop.ps1。
"""
from __future__ import annotations

import argparse
from pathlib import Path

from PIL import Image, ImageDraw

NAVY = (14, 30, 58, 255)        # 底色：深藍
TILE = (60, 92, 140, 255)       # 未亮起的格子：灰藍
AMBER = (245, 166, 35, 255)     # 亮起的格子：琥珀
SUPER = 4                       # 超取樣倍率，抗鋸齒


def render(size: int) -> Image.Image:
    """以超取樣畫一張 size×size 的圖示。幾何比例依尺寸微調，讓小圖的間距不會糊掉。"""
    s = size * SUPER
    img = Image.new("RGBA", (s, s), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)

    # 底：圓角方形（留 2% 邊透明，避免 Windows 貼邊）
    pad = round(s * 0.02)
    bg_r = round(s * 0.22)
    d.rounded_rectangle((pad, pad, s - pad - 1, s - pad - 1), radius=bg_r, fill=NAVY)

    # 2×2 方格：小尺寸加大間距比例，確保縮到 16px 仍有 ≥1px 的暗縫
    if size <= 24:
        inset, gap = 0.20, 0.12
    elif size <= 48:
        inset, gap = 0.20, 0.10
    else:
        inset, gap = 0.21, 0.08
    inset_px = s * inset
    gap_px = s * gap
    cell = (s - 2 * inset_px - gap_px) / 2
    tile_r = max(round(cell * 0.22), SUPER)

    for row in range(2):
        for col in range(2):
            x0 = inset_px + col * (cell + gap_px)
            y0 = inset_px + row * (cell + gap_px)
            lit = row == 0 and col == 0
            d.rounded_rectangle(
                (x0, y0, x0 + cell, y0 + cell),
                radius=tile_r,
                fill=AMBER if lit else TILE,
            )

    return img.resize((size, size), Image.LANCZOS)


PNG_FILES = {
    "32x32.png": 32,
    "64x64.png": 64,
    "128x128.png": 128,
    "128x128@2x.png": 256,
    "icon.png": 512,
    "StoreLogo.png": 50,
    "Square30x30Logo.png": 30,
    "Square44x44Logo.png": 44,
    "Square71x71Logo.png": 71,
    "Square89x89Logo.png": 89,
    "Square107x107Logo.png": 107,
    "Square142x142Logo.png": 142,
    "Square150x150Logo.png": 150,
    "Square284x284Logo.png": 284,
    "Square310x310Logo.png": 310,
}
ICO_SIZES = [16, 24, 32, 48, 64, 256]


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--out", type=Path, default=Path(__file__).resolve().parents[1] / "src-tauri" / "icons")
    args = ap.parse_args()
    out: Path = args.out
    out.mkdir(parents=True, exist_ok=True)

    for name, size in PNG_FILES.items():
        render(size).save(out / name, format="PNG", optimize=True)
        print(f"wrote {out / name} ({size}px)")

    # icon.ico：每個尺寸各自渲染（不是由大圖縮小），小圖才清楚
    # Pillow 會丟掉比基底圖大的尺寸，所以基底用最大的 256px，其餘以 append_images 附上
    frames = [render(sz) for sz in sorted(ICO_SIZES, reverse=True)]
    frames[0].save(
        out / "icon.ico",
        format="ICO",
        sizes=[(sz, sz) for sz in ICO_SIZES],
        append_images=frames[1:],
    )
    print(f"wrote {out / 'icon.ico'} sizes={ICO_SIZES}")


if __name__ == "__main__":
    main()
