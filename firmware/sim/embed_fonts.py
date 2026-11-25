"""Writes the board's TTF fonts as C arrays (ui_fonts.h, HUB_SIM): embed_fonts.py <fonts dir> <out.c>."""
import sys

src, out = sys.argv[1], sys.argv[2]
fonts = [("icons", "MaterialIcons-Regular.ttf"), ("text", "Inter-Medium.ttf"), ("clock", "Inter-SemiBold.ttf")]
with open(out, "w") as f:
    f.write("#include <stdint.h>\n")
    for name, file in fonts:
        data = open(f"{src}/{file}", "rb").read()
        f.write(f"const uint8_t sim_{name}_ttf[] = {{\n")
        for i in range(0, len(data), 24):
            f.write(",".join(str(b) for b in data[i:i + 24]) + ",\n")
        f.write("};\n")
        f.write(f"const unsigned sim_{name}_ttf_len = {len(data)};\n")
