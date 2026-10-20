#!/usr/bin/env python3
"""把 `cargo run --example dump_screen` 输出的 JSON 渲染成 PNG。

走 TestBackend 而不是解析 ANSI 转义序列：拿到的就是 ratatui 真正画出来的格子，
不会因为光标定位/全角字符宽度算错而串行。

用法：

    cd crates/kanban-tui
    cargo run --example dump_screen -- /path/to/kanban.db 96 30 > /tmp/board.json
    cargo run --example dump_screen -- /path/to/kanban.db 96 30 '\\r' > /tmp/detail.json
    python3 ../../docs/make-screenshot.py /tmp/board.json ../../docs/screenshot.png

依赖 Pillow（brew install pillow）。
"""

import json
import sys

from PIL import Image, ImageDraw, ImageFont

MONO = "/System/Library/Fonts/Menlo.ttc"
CJK = "/System/Library/Fonts/Hiragino Sans GB.ttc"
BG_WINDOW = (17, 20, 27)
BG_OUTER = (13, 15, 20)


def wide(cp):
    return (
        0x1100 <= cp <= 0x115F
        or 0x2E80 <= cp <= 0xA4CF
        or 0xAC00 <= cp <= 0xD7A3
        or 0xF900 <= cp <= 0xFAFF
        or 0xFE30 <= cp <= 0xFE6F
        or 0xFF00 <= cp <= 0xFF60
        or 0xFFE0 <= cp <= 0xFFE6
        or 0x1F300 <= cp <= 0x1FAFF
        or 0x2600 <= cp <= 0x27BF
    )


def load(path):
    with open(path) as fh:
        data = json.load(fh)
    cells = data["cells"]
    cols = max(c[0] for c in cells) + 1
    rows = max(c[1] for c in cells) + 1
    grid = [[None] * cols for _ in range(rows)]
    for x, y, sym, fr, fg, fb, bg, bold, crossed in cells:
        grid[y][x] = {
            "sym": sym,
            "fg": (fr, fg, fb),
            "bg": tuple(bg) if bg else None,
            "bold": bool(bold),
            "crossed": bool(crossed),
        }
    return grid, cols, rows


def main():
    src, out = sys.argv[1], sys.argv[2]
    grid, cols, rows = load(src)

    size = 15
    mono = ImageFont.truetype(MONO, size, index=0)
    mono_b = ImageFont.truetype(MONO, size, index=1)
    cell_w = int(round(mono.getlength("M")))
    cell_h = int(round(size * 1.42))

    # 让全角字宽度正好等于两格
    cjk_size = size
    while cjk_size < size * 2:
        probe = ImageFont.truetype(CJK, cjk_size, index=0)
        if probe.getlength("中") >= cell_w * 2:
            break
        cjk_size += 1
    cjk = ImageFont.truetype(CJK, cjk_size, index=0)

    pad, titlebar = 22, 30
    W = cols * cell_w + pad * 2
    H = rows * cell_h + pad * 2 + titlebar

    img = Image.new("RGB", (W, H), BG_OUTER)
    d = ImageDraw.Draw(img)
    d.rounded_rectangle(
        [pad // 2, pad // 2, W - pad // 2, H - pad // 2],
        radius=12,
        fill=BG_WINDOW,
        outline=(42, 48, 60),
    )
    for i, color in enumerate([(255, 95, 86), (255, 189, 46), (39, 201, 63)]):
        cx = pad // 2 + 18 + i * 20
        d.ellipse([cx - 6, 6 + pad // 2, cx + 6, 18 + pad // 2], fill=color)

    ox, oy = pad, pad + titlebar

    for y in range(rows):
        x = 0
        while x < cols:
            cell = grid[y][x]
            if cell is None:
                x += 1
                continue
            sym = cell["sym"]
            ch = sym[0] if sym else " "
            is_wide = wide(ord(ch))
            cw = cell_w * (2 if is_wide else 1)
            px, py = ox + x * cell_w, oy + y * cell_h

            # 全角字两格共用一个底色
            if cell["bg"]:
                d.rectangle([px, py, px + cw - 1, py + cell_h - 1], fill=cell["bg"])
            if ch != " ":
                if is_wide:
                    gw = cjk.getlength(ch)
                    d.text((px + (cw - gw) / 2, py + 1), ch, font=cjk, fill=cell["fg"])
                else:
                    font = mono_b if cell["bold"] else mono
                    d.text((px, py + 1), ch, font=font, fill=cell["fg"])
                if cell["crossed"]:
                    d.line(
                        [px, py + cell_h // 2, px + cw - 1, py + cell_h // 2],
                        fill=cell["fg"],
                    )
            x += 2 if is_wide else 1

    img.save(out)
    print(f"{out}  {W}x{H}")


if __name__ == "__main__":
    main()
