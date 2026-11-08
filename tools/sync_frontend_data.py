#!/usr/bin/env python3
"""Refresh data copied from a ComfyUI frontend checkout (default ../ComfyUI_frontend-1.60.0).

  tools/sync_frontend_data.py [/path/to/ComfyUI_frontend-x.y.z]

Copies the node-usage frequency map that orders "Most relevant" in the Add Node search.
The search *logic* is ported by hand in src/nodesearch/ — see AGENT-HANDOFF.md, "Re-syncing".
"""
import shutil, sys, pathlib

root = pathlib.Path(__file__).resolve().parent.parent
fe = pathlib.Path(sys.argv[1]) if len(sys.argv) > 1 else pathlib.Path.home() / "Documents/ComfyUI_frontend-1.60.0"
shutil.copy(fe / "public/assets/sorted-custom-node-map.json", root / "data/frontend/node-frequency.json")
print("updated data/frontend/node-frequency.json from", fe)
