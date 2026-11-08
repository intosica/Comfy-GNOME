#!/usr/bin/env python3
"""Simulated backend for README screenshots: real ComfyUI routes the app uses, fake data.

    python3 tools/demo/mock_backend.py [--port 8199]

Deterministic data only; nothing here touches a real ComfyUI install. The graph is a small
SDXL img2img-ish workflow plus one subgraph ("Face Detailer"), so the UI shows every kind of
widget, a typed wire bundle, a subgraph and a runnable queue.
"""
import argparse
import base64
import hashlib
import itertools
import json
import math
import struct
import time
import zlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, unquote, urlparse

NODES = {
    "CheckpointLoaderSimple": dict(cat="loaders", out=["MODEL", "CLIP", "VAE"],
                                   req={"ckpt_name": [["dreamshaperXL_v21TurboDPMSDE.safetensors",
                                                       "sd_xl_base_1.0.safetensors"]]}),
    "CLIPTextEncode": dict(cat="conditioning", out=["CONDITIONING"], req={"text": ["STRING", {"multiline": True}]},
                           opt={"clip": ["CLIP"]}),
    "EmptyLatentImage": dict(cat="latent", out=["LATENT"],
                             req={"width": ["INT", {"default": 1024, "min": 16, "max": 8192}],
                                  "height": ["INT", {"default": 1024, "min": 16, "max": 8192}],
                                  "batch_size": ["INT", {"default": 1, "min": 1, "max": 64}]}),
    "KSampler": dict(cat="sampling", out=["LATENT"],
                     req={"model": ["MODEL"], "seed": ["INT", {"default": 0, "min": 0, "max": 2**32}],
                          "steps": ["INT", {"default": 20, "min": 1, "max": 150}],
                          "cfg": ["FLOAT", {"default": 8.0, "min": 0.0, "max": 30.0, "step": 0.1}],
                          "sampler_name": [["euler", "euler_ancestral", "dpmpp_2m", "dpmpp_sde"]],
                          "scheduler": [["normal", "karras", "exponential"]],
                          "positive": ["CONDITIONING"], "negative": ["CONDITIONING"],
                          "latent_image": ["LATENT"], "denoise": ["FLOAT", {"default": 1.0, "min": 0.0, "max": 1.0, "step": 0.01}]}),
    "VAEDecode": dict(cat="latent", out=["IMAGE"], req={"samples": ["LATENT"], "vae": ["VAE"]}),
    "SaveImage": dict(cat="image", out=[], req={"images": ["IMAGE"], "filename_prefix": ["STRING", {"default": "ComfyUI"}]}),
    "LoraLoader": dict(cat="loaders", out=["MODEL", "CLIP"],
                       req={"model": ["MODEL"], "clip": ["CLIP"],
                            "lora_name": [["anime_linework_xl.safetensors", "film_grain_xl.safetensors", "detail_slider_xl.safetensors"]],
                            "strength_model": ["FLOAT", {"default": 1.0, "min": -10.0, "max": 10.0, "step": 0.01}],
                            "strength_clip": ["FLOAT", {"default": 1.0, "min": -10.0, "max": 10.0, "step": 0.01}]}),
    "easy wildcards": dict(cat="utils", out=["STRING"],
                           req={"text": ["STRING", {"multiline": True}],
                                "Select the LoRA to add to the text": [["Select the LoRA to add to the text", "anime_linework_xl.safetensors"]],
                                "Select the Wildcard to add to the text": [["Select the Wildcard to add to the text", "__lighting__", "__style__"]],
                                "seed": ["INT", {"default": 0, "min": 0, "max": 2**32}]}),
    "FaceDetailerSubgraph": dict(cat="subgraph", out=["IMAGE"], req={"image": ["IMAGE"], "model": ["MODEL"]}),
    "Power Lora Loader (rgthree)": dict(cat="loaders", out=["MODEL", "CLIP"], req={},
                                       opt={"model": ["MODEL"], "clip": ["CLIP"]}),
}

TEMPLATES = ["Text to image", "Image to image", "SDXL turbo", "Upscale 2×", "Inpaint"]
SUBGRAPH_ID = "b1f4c2a7-5e3d-4a9c-9b21-77d0c15aa3e1"

NODE_TEMPLATES = {
    "Text to image": "2026-01-04_09-12-01 ComfyUI_0007",
    "Image to image": "img2img_soft_light",
    "SDXL turbo": "sdxl_turbo_1024",
    "Upscale 2×": "upscale_x2_lanczos",
    "Inpaint": "inpaint_face_region",
}


def link(i, o, os_, t, ts, ty):
    return [i, o, os_, t, ts, ty]


def graph_for(name):
    """A 9-node SDXL workflow; `name` only changes the prompt text and the missing-input node."""
    sub = name == "Face Detailer"
    nodes = [
        dict(id=1, type="CheckpointLoaderSimple", pos=[40, 120], size=[315, 98], mode=0,
             outputs=[dict(name="MODEL", type="MODEL", links=[1]), dict(name="CLIP", type="CLIP", links=[2, 3]),
                      dict(name="VAE", type="VAE", links=[4])], widgets_values=["dreamshaperXL_v21TurboDPMSDE.safetensors"]),
        dict(id=2, type="CLIPTextEncode", pos=[400, 60], size=[420, 170], mode=0,
             inputs=[dict(name="clip", type="CLIP", link=2)],
             outputs=[dict(name="CONDITIONING", type="CONDITIONING", links=[5])],
             widgets_values=["a quiet harbour at dawn, (soft light:1.15), film grain, 35 mm\n"
                             "wide shot, pastel sky, small fishing boats"]),
        dict(id=3, type="CLIPTextEncode", pos=[400, 280], size=[420, 120], mode=0,
             inputs=[dict(name="clip", type="CLIP", link=3)],
             outputs=[dict(name="CONDITIONING", type="CONDITIONING", links=[6])],
             widgets_values=["blurry, watermark, text, extra fingers"]),
        dict(id=4, type="EmptyLatentImage", pos=[400, 440], size=[315, 106], mode=0,
             outputs=[dict(name="LATENT", type="LATENT", links=[7])], widgets_values=[1024, 1024, 1]),
        dict(id=5, type="KSampler", pos=[900, 110], size=[315, 262], mode=0,
             inputs=[dict(name="model", type="MODEL", link=1), dict(name="positive", type="CONDITIONING", link=5),
                     dict(name="negative", type="CONDITIONING", link=6), dict(name="latent_image", type="LATENT", link=7)],
             outputs=[dict(name="LATENT", type="LATENT", links=[8])],
             widgets_values=[42, "randomize", 24, 7.5, "dpmpp_2m", "karras", 1.0]),
        dict(id=6, type="VAEDecode", pos=[1240, 120], size=[210, 46], mode=0,
             inputs=[dict(name="samples", type="LATENT", link=8), dict(name="vae", type="VAE", link=4)],
             outputs=[dict(name="IMAGE", type="IMAGE", links=[9])]),
        dict(id=7, type="SaveImage", pos=[1240, 220], size=[300, 230], mode=0,
             inputs=[dict(name="images", type="IMAGE", link=9)], widgets_values=["harbour"]),
        dict(id=8, type="SubgraphInstance", pos=[880, 470], size=[315, 74], mode=4,
             inputs=[dict(name="image", type="IMAGE", link=None), dict(name="model", type="MODEL", link=None)],
             outputs=[dict(name="IMAGE", type="IMAGE", links=[])], title="Face Detailer"),
        dict(id=10, type="Power Lora Loader (rgthree)", pos=[40, 630], size=[315, 200], mode=0,
             inputs=[dict(name="model", type="MODEL", link=1), dict(name="clip", type="CLIP", link=None)],
             outputs=[dict(name="MODEL", type="MODEL", links=[11]), dict(name="CLIP", type="CLIP", links=[])],
             widgets_values=[{}, {"type": "PowerLoraLoaderHeaderWidget"},
                             {"on": True, "lora": "anime_linework_xl.safetensors", "strength": 0.7},
                             {"on": True, "lora": "film_grain_xl.safetensors", "strength": 0.35}, "", ""]),
        dict(id=9, type="easy wildcards", pos=[40, 300], size=[315, 300], mode=0,
             outputs=[dict(name="STRING", type="STRING", links=[10])],
             widgets_values=["__style__, __lighting__", "Select the LoRA to add to the text",
                             "Select the Wildcard to add to the text", 0, "randomize"]),
    ]
    links = [link(1, 1, 0, 10, 0, "MODEL"), link(11, 10, 0, 5, 0, "MODEL"),
             link(2, 1, 1, 2, 0, "CLIP"), link(3, 1, 1, 3, 0, "CLIP"),
             link(4, 1, 2, 6, 1, "VAE"), link(5, 2, 0, 5, 1, "CONDITIONING"), link(6, 3, 0, 5, 2, "CONDITIONING"),
             link(7, 4, 0, 5, 3, "LATENT"), link(8, 5, 0, 6, 0, "LATENT"), link(9, 6, 0, 7, 0, "IMAGE")]
    nodes[7]["type"] = SUBGRAPH_ID  # always a subgraph instance so drilling in works
    wf = dict(last_node_id=10, last_link_id=11, nodes=nodes, links=links, groups=[
        dict(title="Prompt", bounding=[393, 70, 440, 345], color="#3f789e", font_size=24, flags={})],
        config={}, extra=dict(ds=dict(scale=0.85, offset=[40, 10])), version=0.4)
    if True:
        inner = dict(last_node_id=3, last_link_id=3, nodes=[
            dict(id=1, type="KSampler", pos=[0, 0], size=[315, 262], mode=0,
                 inputs=[dict(name="model", type="MODEL", link=None), dict(name="positive", type="CONDITIONING", link=None),
                         dict(name="negative", type="CONDITIONING", link=None), dict(name="latent_image", type="LATENT", link=None)],
                 widgets_values=[7, "fixed", 12, 4.5, "euler", "normal", 0.45]),
            dict(id=2, type="VAEDecode", pos=[380, 0], size=[210, 46], mode=0),
        ], links=[], groups=[])
        # A subgraph definition holds its graph inline (nodes/links/groups), like ComfyUI's own files.
        wf["definitions"] = dict(subgraphs=[dict(inner, id=SUBGRAPH_ID, name="Face Detailer",
                                                 inputNode=dict(bounding=[0, -80, 315, 80]),
                                                 outputNode=dict(bounding=[380, 200, 210, 60]),
                                                 inputs=[dict(name="image", type="IMAGE", linkIds=[])],
                                                 outputs=[dict(name="IMAGE", type="IMAGE", linkIds=[])])])
    return wf


def node_info():
    out = {}
    for name, d in NODES.items():
        req = {k: (v if isinstance(v, list) and v and isinstance(v[0], str) and len(v) == 2 and isinstance(v[1], dict)
                   else v) for k, v in d["req"].items()}
        out[name] = dict(name=name, display_name=name, category=d["cat"], description="",
                         input=dict(required=req, optional=d.get("opt", {})),
                         input_order=dict(required=list(req)), output=d["out"],
                         output_is_list=[False] * len(d["out"]), output_name=d["out"],
                         python_module="", output_node=not d["out"])
    return out


def png(w, h, hue):
    """A small RGB gradient PNG (no PIL dependency)."""
    rows = bytearray()
    for y in range(h):
        rows.append(0)
        for x in range(w):
            r = int(40 + 120 * (x / w))
            g = int(40 + 100 * (y / h))
            b = int(60 + 90 * hue + 60 * math.sin((x + y) / 22.0))
            rows += bytes((r % 256, g % 256, b % 256))
    def chunk(t, data):
        c = struct.pack(">I", len(data)) + t + data
        return c + struct.pack(">I", zlib.crc32(t + data) & 0xFFFFFFFF)
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(bytes(rows), 6)) + chunk(b"IEND", b""))


STATE = dict(queue=[], history={}, counter=0)
PROMPTS = [
    ("3f9c11aa-0b6e-4a3e-9c02-5f8d1b7e5a10", 100.0, "harbour"),
    ("b2d40c77-1a9d-4f21-8a63-2ee71c4b9d38", 74.5, "harbour_seed41"),
]


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *a):
        pass

    def send(self, code, body, ctype="application/json"):
        if isinstance(body, (dict, list)):
            body = json.dumps(body).encode()
        self.send_response(code)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def ws_send(self, opcode, payload):
        """Server-to-client frame (unmasked, as RFC 6455 requires for servers)."""
        hdr = bytes([0x80 | opcode])
        n = len(payload)
        if n < 126:
            hdr += bytes([n])
        elif n < 65536:
            hdr += b"\x7e" + struct.pack(">H", n)
        else:
            hdr += b"\x7f" + struct.pack(">Q", n)
        self.wfile.write(hdr + payload)
        self.wfile.flush()

    def do_GET(self):
        u = urlparse(self.path)
        p, q = unquote(u.path), parse_qs(u.query)
        if p in ("/", "/index.html"):
            return self.send(200, b"<!doctype html><title>mock</title><p>mock ComfyUI", "text/html")
        if p == "/ws":
            # Minimal RFC 6455 upgrade; the socket stays open so the app sees a live connection.
            key = self.headers.get("Sec-WebSocket-Key", "")
            accept = base64.b64encode(hashlib.sha1((key + "258EAFA5-E914-47DA-95CA-C5AB0DC85B11").encode()).digest())
            self.send_response(101, "Switching Protocols")
            self.send_header("Upgrade", "websocket")
            self.send_header("Connection", "Upgrade")
            self.send_header("Sec-WebSocket-Accept", accept.decode())
            self.end_headers()
            self.wfile.flush()
            try:
                if not PREVIEW:
                    while True:
                        time.sleep(30)
                        self.wfile.write(b"")  # keep-alive; client pings are fine to ignore
                # Simulated sampling: progress + type-4 frames on the KSampler (node 5),
                # exactly what the real backend sends once the feature-flag handshake lands.
                pid = "demo-live"
                for step in itertools.cycle(range(1, 21)):
                    self.ws_send(1, json.dumps({"type": "executing", "data": {"node": "5", "prompt_id": pid}}).encode())
                    self.ws_send(1, json.dumps({"type": "progress_state", "data": {"nodes": {
                        "5": {"state": "running", "value": step, "max": 20}}}}).encode())
                    meta = json.dumps({"node_id": "5", "display_node_id": "5", "parent_node_id": None,
                                       "real_node_id": "5", "prompt_id": pid}).encode()
                    self.ws_send(2, struct.pack(">II", 4, len(meta)) + meta + png(256, 160, 0.55 + step / 60.0))
                    time.sleep(0.25)
            except Exception:
                return
        if p == "/system_stats":
            return self.send(200, dict(system=dict(os="posix", comfyui_version="0.3.40", python_version="3.12.4",
                                                    pytorch_version="2.5.1", ram_total=68719476736, ram_free=25769803776),
                                       devices=[dict(name="cuda:0 NVIDIA GeForce RTX 4090", type="cuda", index=0)]))
        if p == "/object_info":
            return self.send(200, node_info())
        if p == "/api/userdata":
            return self.send(200, [dict(path=f"{n}.json", size=4200, modified=1767225600) for n in
                                   [TEMPLATES[0], "Anima - Reference", "harbour_sdxl", "portrait_lighting"]])
        if p.startswith("/api/userdata/"):
            name = p.rsplit("/", 1)[-1]
            return self.send(200, graph_for(name), "application/json")
        if p == "/api/settings":
            return self.send(200, {"Comfy.Queue.History.Expanded": True, "Comfy.Workflow.AutoSave": "off",
                                   "Comfy.Locale": "en", "LiteGraph.Canvas.MaximumFps": 60,
                                   "Comfy.SnapToGrid.GridSize": 20, "pysssss.SnapToGrid": False,
                                   "Comfy.LinkRenderMode": 2})
        if p == "/queue":
            # Real wire shape: entries are [number, prompt_id, prompt, extra_data, outputs].
            def entry(i):
                return [i, f"demo-{i}", {"1": {"class_type": "KSampler"}}, {}, ["7"]]
            pending = [entry(i) for i in range(1, STATE["counter"] + 1)]
            return self.send(200, dict(queue_running=[], queue_pending=pending))
        if p == "/history":
            items = {}
            for pid, dur, prefix in PROMPTS:
                stamps = [[f"harbour_{i:05d}_.png", {"workflow": graph_for("harbour"), "prompt": {}}] for i in (3, 2, 1)]
                items[pid] = dict(prompt=[0, pid, {}, {"client_id": "comfy-gnome-demo"}, ["7"]],
                                  outputs={"7": dict(images=stamps)}, status=dict(status_str="success", completed=True,
                                                                                  messages=[["execution_start", {"timestamp": 0}]]),
                                  meta=dict())
            return self.send(200, items)
        if p == "/history/":
            return self.send(200, {})
        if p == "/experiment/models":
            return self.send(200, [dict(name="checkpoints", folders=["models/checkpoints"], extensions=[".safetensors"]),
                                   dict(name="loras", folders=["models/loras"], extensions=[".safetensors"])])
        m = p.rsplit("/", 1)
        if p.startswith("/experiment/models/"):
            return self.send(200, [dict(name="dreamshaperXL_v21TurboDPMSDE.safetensors", pathIndex=0, size=1, modified=0),
                                   dict(name="sd_xl_base_1.0.safetensors", pathIndex=0, size=1, modified=0)]
                             if m[-1] == "checkpoints" else
                             [dict(name="anime_linework_xl.safetensors", pathIndex=0, size=1, modified=0)])
        if p.startswith("/api/lm/") and p.endswith("/preview-url"):
            return self.send(200, dict(success=True, preview_url="/demo/preview.png"))
        if p == "/demo/preview.png" or p.startswith("/view"):
            return self.send(200, png(320, 448, 0.5), "image/png")
        if p == "/extensions":
            return self.send(200, ["easy-use", "rgthree-comfy", "comfyui-lora-manager", "KJNodes"])
        return self.send(404, dict(error="not found", path=p))

    def do_POST(self):
        p = unquote(urlparse(self.path).path)
        n = int(self.headers.get("Content-Length") or 0)
        body = json.loads(self.rfile.read(n) or b"{}")
        if p == "/prompt":
            STATE["counter"] += 1
            pid = f"demo-{STATE['counter']}"
            print(f"[mock] prompt {pid}", flush=True)
            return self.send(200, dict(prompt_id=pid, number=STATE["counter"], node_errors={}))
        if p == "/interrupt" or p.startswith("/api/settings/") or p == "/history":
            return self.send(200, {})
        if p.startswith("/api/userdata/"):
            return self.send(200, {})
        return self.send(404, dict(error="not found", path=p))


if __name__ == "__main__":
    ap = argparse.ArgumentParser()
    ap.add_argument("--port", type=int, default=8199)
    ap.add_argument("--preview", action="store_true", help="stream simulated sampler progress + preview frames")
    a = ap.parse_args()
    PREVIEW = a.preview
    print(f"mock ComfyUI on http://127.0.0.1:{a.port}" + (" (preview simulation)" if PREVIEW else ""), flush=True)
    ThreadingHTTPServer(("127.0.0.1", a.port), Handler).serve_forever()
