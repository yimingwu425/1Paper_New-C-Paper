#!/usr/bin/env python3
"""Generate platform containers from approved art; never redraw the source."""
import argparse, hashlib, json, struct, subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
ICONS = ROOT / "desktop/icons/v2"
PAIRS = [("icp4",16),("icp5",32),("icp6",64),("ic07",128),("ic08",256),("ic09",512),("ic10",1024),("ic11",32),("ic12",64),("ic13",256),("ic14",512)]

def png_size(raw):
    assert raw[:8] == b"\x89PNG\r\n\x1a\n", "Not a PNG"
    width, height, depth, color = struct.unpack(">IIBB", raw[16:26])
    assert depth == 8 and color == 6, "Expected 8-bit RGBA PNG"
    return width, height

def check():
    manifest = json.loads((ICONS / "manifest.json").read_text())
    for name, digest in manifest["sha256"].items():
        assert hashlib.sha256((ICONS/name).read_bytes()).hexdigest() == digest, name
    for size in [16,32,64,128,256,512,1024]:
        assert png_size((ICONS/f"{size}x{size}.png").read_bytes()) == (size,size)
    assert (ICONS/"icon.png").read_bytes() == (ICONS/"1024x1024.png").read_bytes()
    assert (ICONS/"128x128@2x.png").read_bytes() == (ICONS/"256x256.png").read_bytes()
    data = (ICONS/"icon.icns").read_bytes()
    assert data[:4] == b"icns" and struct.unpack(">I",data[4:8])[0] == len(data)
    at = 8
    for kind, size in PAIRS:
        assert data[at:at+4] == kind.encode()
        length = struct.unpack(">I", data[at+4:at+8])[0]
        assert data[at+8:at+length] == (ICONS/f"{size}x{size}.png").read_bytes()
        at += length
    assert at == len(data)
    ico = (ICONS/"icon.ico").read_bytes()
    assert ico[:6] == struct.pack("<HHH",0,1,5)
    actual = []
    for i in range(5):
        width,height,_,_,_,_,length,offset = struct.unpack("<BBBBHHII",ico[6+i*16:22+i*16])
        actual.append((width or 256,height or 256))
        assert length > 0 and offset+length <= len(ico)
    assert actual == [(s,s) for s in [16,32,64,128,256]]
    config = json.loads((ROOT/"desktop/tauri.conf.json").read_text())
    for name in config["bundle"]["icon"]:
        assert (ROOT/"desktop"/name).is_file(), name
    assert "icons/v2/icon.icns" in config["bundle"]["icon"]
    for size, name in [(32,"favicon-32.png"),(256,"app-icon.png")]:
        assert (ROOT/"web/public/icons/v2"/name).read_bytes() == (ICONS/f"{size}x{size}.png").read_bytes()
    assert '/icons/v2/favicon-32.png' in (ROOT/"web/index.html").read_text()
    print("PASS: PNG sizes/RGBA, SHA-256, all 11 ICNS entries, ICO sizes, Tauri paths, favicon and brand assets.")

def generate():
    for size in [16,32,64,128,256,512,1024]:
        subprocess.run(["magick",str(ICONS/"source-approved.png"),"-filter","Lanczos","-resize",f"{size}x{size}","-channel","A","-fx","u<=1/255?0:u>=254/255?1:u","+channel","-depth","8","-define","png:color-type=6",str(ICONS/f"{size}x{size}.png")],check=True)
    (ICONS/"icon.png").write_bytes((ICONS/"1024x1024.png").read_bytes())
    (ICONS/"128x128@2x.png").write_bytes((ICONS/"256x256.png").read_bytes())
    chunks = []
    for kind,size in PAIRS:
        raw = (ICONS/f"{size}x{size}.png").read_bytes()
        chunks.append(kind.encode()+struct.pack(">I",len(raw)+8)+raw)
    payload = b"".join(chunks)
    (ICONS/"icon.icns").write_bytes(b"icns"+struct.pack(">I",len(payload)+8)+payload)
    subprocess.run(["magick",*[str(ICONS/f"{s}x{s}.png") for s in [16,32,64,128,256]],str(ICONS/"icon.ico")],check=True)
    dest = ROOT/"web/public/icons/v2"
    dest.mkdir(parents=True,exist_ok=True)
    for size,name in [(32,"favicon-32.png"),(256,"app-icon.png")]:
        (dest/name).write_bytes((ICONS/f"{size}x{size}.png").read_bytes())
    files = {p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in sorted(ICONS.iterdir()) if p.is_file() and p.name != "manifest.json"}
    (ICONS/"manifest.json").write_text(json.dumps({"version":"2026-11-10-approved-v2","source":"User-approved icon design; source-approved.png retained unchanged.","normalization":"Lanczos size variants; alpha 0 or 1 becomes 0, alpha 254 or 255 becomes 255. No artwork changes.","sha256":files},indent=2)+"\n")

if __name__ == "__main__":
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--check",action="store_true",help="Read-only validation; no ImageMagick needed.")
    args = p.parse_args()
    if not args.check:
        generate()
    check()

