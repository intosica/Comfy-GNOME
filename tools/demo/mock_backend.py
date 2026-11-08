#!/usr/bin/env python3
"""Simulated backend for README screenshots: real ComfyUI routes the app uses, fake data.

    python3 tools/demo/mock_backend.py [--port 8199]

Deterministic data only; nothing here touches a real ComfyUI install. The graph is a small
Anima text-to-image workflow (tools/demo/workflow.json, taken from ComfyUI_00004.png) with a subgraph
definition, so the UI shows combo/number/text widgets, typed wires and a runnable queue.
"""
import argparse
import base64
import hashlib
import itertools
import json
import os
import struct
import time
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
                          "sampler_name": [["euler", "euler_ancestral", "dpmpp_2m", "dpmpp_sde", "exp_heun_2_x0"]],
                          "scheduler": [["normal", "karras", "exponential", "beta"]],
                          "positive": ["CONDITIONING"], "negative": ["CONDITIONING"],
                          "latent_image": ["LATENT"], "denoise": ["FLOAT", {"default": 1.0, "min": 0.0, "max": 1.0, "step": 0.01}]}),
    "VAEDecode": dict(cat="latent", out=["IMAGE"], req={"samples": ["LATENT"], "vae": ["VAE"]}),
    "SaveImage": dict(cat="image", out=[], req={"images": ["IMAGE"], "filename_prefix": ["STRING", {"default": "ComfyUI"}]}),
    # Target of "Open in New Workflow" and of a dropped picture (`image_upload` adds the picker).
    "LoadImage": dict(cat="image", out=["IMAGE", "MASK"],
                      req={"image": [["example.png", "harbour_00003_.png"], {"image_upload": True}]}),
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
    "UNETLoader": dict(cat="advanced/loaders", out=["MODEL"],
                       req={"unet_name": [["anima-base-v1.0.safetensors", "flux1-dev.safetensors"]],
                            "weight_dtype": [["default", "fp8_e4m3fn", "fp8_e5m2"]]}),
    "VAELoader": dict(cat="loaders", out=["VAE"], req={"vae_name": [["qwen_image_vae.safetensors", "ae.safetensors"]]}),
    "CLIPLoader": dict(cat="advanced/loaders", out=["CLIP"],
                       req={"clip_name": [["qwen_3_06b_base.safetensors", "t5xxl_fp16.safetensors"]],
                            "type": [["stable_diffusion", "stable_cascade", "sd3", "flux"]],
                            "device": [["default", "cpu"]]}, opt={}),
    "SaveImageAdvanced": dict(cat="image", out=[],
                              req={"images": ["IMAGE"], "filename_prefix": ["STRING", {"default": "ComfyUI"}],
                                   "format": [["png", "jpg", "webp"]], "bit_depth": [["8-bit", "16-bit"]],
                                   "color_space": [["sRGB", "Display P3"]]}),
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


def graph_for(name):
    """The screenshot workflow: ComfyUI_00004.png's embedded graph (tools/demo/workflow.json), whatever the name."""
    with open(os.path.join(os.path.dirname(os.path.abspath(__file__)), "workflow.json")) as f:
        return json.load(f)


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


# The simulated result: ComfyUI_00004.png (the workflow's own output), downsized to 512 px.
RESULT = open(os.path.join(os.path.dirname(__file__), "result.jpg"), "rb").read()

STATE = dict(queue=[], history={}, counter=0)
NOW = time.time()
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
                # Simulated sampling: progress + type-4 frames on the KSampler (node 462),
                # exactly what the real backend sends once the feature-flag handshake lands.
                pid = "demo-live"
                for step in itertools.cycle(range(1, 21)):
                    self.ws_send(1, json.dumps({"type": "executing", "data": {"node": "462", "prompt_id": pid}}).encode())
                    self.ws_send(1, json.dumps({"type": "progress_state", "data": {"nodes": {
                        "462": {"state": "running", "value": step, "max": 20}}}}).encode())
                    meta = json.dumps({"node_id": "462", "display_node_id": "462", "parent_node_id": None,
                                       "real_node_id": "462", "prompt_id": pid}).encode()
                    self.ws_send(2, struct.pack(">II", 4, len(meta)) + meta + RESULT)
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
                                   [TEMPLATES[0], "Anima - Reference", "ComfyUI_00004", "portrait_lighting"]])
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
                return [i, f"demo-{i}", {"1": {"class_type": "KSampler"}}, {}, ["469"]]
            pending = [entry(i) for i in range(1, STATE["counter"] + 1)]
            return self.send(200, dict(queue_running=[], queue_pending=pending))
        if p == "/history":
            items = {}
            for pid, dur, prefix in PROMPTS:
                # Real wire shape: one object per image (`/history` nests them under
                # outputs[node].images); a list here shows up as an empty filename.
                stamps = [dict(filename=f"harbour_{i:05d}_.png", subfolder="", type="output")
                          for i in (3, 2, 1)]
                end = NOW - 60.0 * (len(items) + 1)
                items[pid] = dict(prompt=[0, pid, {}, {"client_id": "comfy-gnome-demo"}, ["469"]],
                                  outputs={"469": dict(images=stamps)}, status=dict(status_str="success", completed=True,
                                                                                  messages=[["execution_start", {"timestamp": (end - dur) * 1000}],
                                                                                            ["execution_success", {"timestamp": end * 1000}]]),
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
            return self.send(200, RESULT, "image/jpeg")
        if p == "/extensions":
            return self.send(200, ["easy-use", "rgthree-comfy", "comfyui-lora-manager", "KJNodes"])
        return self.send(404, dict(error="not found", path=p))

    def do_POST(self):
        p = unquote(urlparse(self.path).path)
        n = int(self.headers.get("Content-Length") or 0)
        raw = self.rfile.read(n)
        if p == "/upload/image":
            # Multipart form: remember the filename the way ComfyUI's input folder does.
            name = "uploaded.png"
            marker = b'filename="'
            if marker in raw:
                start = raw.index(marker) + len(marker)
                name = raw[start:raw.index(b'"', start)].decode("utf-8", "replace")
            STATE.setdefault("uploads", []).append(name)
            return self.send(200, dict(name=name, subfolder="", type="input"))
        body = json.loads(raw or b"{}")
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
