#!/usr/bin/env python3
"""Draws the Flakyfy snowflake: app/Assets/flakyfy_logo.svg and flakyfy_logo.png (needs pycairo)."""
import math
import cairo

S = 1024
ARM = 380  # centre to arm tip
BRANCHES = [(0.45, 0.26), (0.72, 0.17)]  # (position along the arm, branch length) as fractions of ARM


def draw(ctx):
    ctx.translate(S / 2, S / 2)
    g = cairo.LinearGradient(0, -ARM, 0, ARM)
    g.add_color_stop_rgb(0, 0.40, 0.82, 1.0)  # ice blue
    g.add_color_stop_rgb(1, 0.36, 0.25, 0.85)  # indigo
    ctx.set_source(g)
    ctx.set_line_cap(cairo.LINE_CAP_ROUND)
    ctx.set_line_join(cairo.LINE_JOIN_ROUND)
    for i in range(6):
        ctx.save()
        ctx.rotate(i * math.pi / 3)
        ctx.set_line_width(46)
        ctx.move_to(0, 0)
        ctx.line_to(0, -ARM)
        ctx.stroke()
        ctx.set_line_width(34)
        for pos, ln in BRANCHES:
            y, l = -ARM * pos, ARM * ln
            for sign in (-1, 1):
                ctx.move_to(0, y)
                ctx.line_to(sign * l * math.sin(math.pi / 3), y - l * math.cos(math.pi / 3))
                ctx.stroke()
        ctx.restore()


out = "app/Assets/flakyfy_logo"
with cairo.SVGSurface(out + ".svg", S, S) as svg:
    draw(cairo.Context(svg))
png = cairo.ImageSurface(cairo.FORMAT_ARGB32, S, S)
draw(cairo.Context(png))
png.write_to_png(out + ".png")
