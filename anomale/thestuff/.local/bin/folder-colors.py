
#!/usr/bin/env python3
import sys
import math

# Define the available Papirus folder colors here.
COLORS = {
    "adwaita": "#93C0EA",
    "blue": "#5294E2",
    "breeze": "#57B8EC",
    "carmine": "#A30002",
    "darkcyan": "#45ABB7",
    "green": "#87B158",
    "indigo": "#5C6BC0",
    "nordic": "#81A1C1",
    "palebrown": "#D1BFAE",
    "pink": "#F06292",
    "teal": "#16A085",
    "white": "#E4E4E4",
    "yellow": "#F9BD30",
    "black": "#4F4F4F",
    "bluegrey": "#607D8B",
    "brown": "#AE8E6C",
    "cyan": "#00BCD4",
    "deeporange": "#EB6637",
    "grey": "#8E8E8E",
    "magenta": "#CA71DF",
    "orange": "#EE923A",
    "paleorange": "EECA8F",
    "red": "#E25252",
    "violet": "#7E57C2",
    "yaru": "#973552",
}

def oklab(hex_color):
    h = hex_color.lstrip("#")
    rgb = [int(h[i:i+2], 16) / 255 for i in (0, 2, 4)]

    def linear(c):
        return c / 12.92 if c <= 0.04045 else ((c + 0.055) / 1.055) ** 2.4

    r, g, b = map(linear, rgb)

    l = (0.4122214708*r + 0.5363325363*g + 0.0514459929*b) ** (1/3)
    m = (0.2119034982*r + 0.6806995451*g + 0.1073969566*b) ** (1/3)
    s = (0.0883024619*r + 0.2817188376*g + 0.6299787005*b) ** (1/3)

    return (
        0.2104542553*l + 0.7936177850*m - 0.0040720468*s,
        1.9779984951*l - 2.4285922050*m + 0.4505937099*s,
        0.0259040371*l + 0.7827717662*m - 0.8086757660*s,
    )

target = oklab(sys.argv[1])

def distance(hex_color):
    color = oklab(hex_color)
    return math.dist(target, color)

print(min(COLORS, key=lambda name: distance(COLORS[name])))
