Now lc_align details.
📦 [ACP] Compressed m00002–m00058 → 1 block(s), ~47468 tokens saved.
# Custom-node UI support for the native ComfyUI client: LoRA Manager, LC123, rgthree, system monitor

`CN` = `/home/montel/Documents/ComfyUI/custom_nodes`. This was read-only; no files were changed.

**Calling the routes:** paths shown as `/api/...` are what a native client requests directly. The JavaScript calls `api.fetchApi('/lm/...')`, which the stock frontend turns into `/api/lm/...`.

---

## 1. System monitor: it's DaSiWa

The live CPU/GPU/VRAM/RAM monitor in the top bar comes from **ComfyUI-DaSiWa-Nodes**.

- **UI:** a widget in the top bar, plus a ⚙ settings menu.
  - Extension `"DaSiWa.SystemMonitor"` is registered at `CN/ComfyUI-DaSiWa-Nodes/js/dasiwa_system_monitor.js:398-399`.
  - In `setup()` it finds a place for itself, in this order:
    - the legacy top bar `[data-testid="legacy-topbar-container"] > .flex` (:72);
    - before `button[aria-label="Extensions"]` (:79, :95);
    - otherwise `body`.
  - It can be dragged to float, or docked left or right (:143).
  - The ⚙ menu (:318-370) sets display mode (lite/full), dock position and which widgets show.
  - Settings live in localStorage key `dasiwa.system_monitor.settings`. Sparklines keep 60 samples (`HISTORY_LENGTH=60`).
- **Backend** (`CN/ComfyUI-DaSiWa-Nodes/nodes/nodes_system_monitor.py`):
  - **WS event** `dasiwa.system_monitor`, sent once a second by a background thread (:351). The JS listens at :427.
  - `GET /dasiwa/system-monitor` returns the same snapshot (:359); the JS fetches it once at startup (:439).
  - `GET /dasiwa/system-monitor/gpus` returns the GPU list (:363).
  - It is turned off when the env var `DASWA_SYSTEM_MONITOR` is set to a false-like value (:33-35).
  - Snapshot shape (:312-338); unknown values are `null`:
    ```json
    {"cpu_percent","cpu_count",
     "ram":{"used","total","percent"}, "swap":{"used","total","percent"},
     "disks":[{"path","device","fstype","used","total","percent","read_mb_s","write_mb_s"}],
     "gpus":[{"id":"NVIDIA:0","index","vendor","name","uuid","utilization",
              "memory_used","memory_total","memory_percent","temperature"}]}
    ```
- **Why not the other candidates:**
  - LC123 `lc_system_check.js:501` only shows a one-off modal report (`.lc-sys-overlay` :350), not a live monitor.
  - midfr's resource_reaper adds a sidebar tab called **"Memory"**, not a top-bar widget.
  - comfyui-easy-use only ships CSS for Crystools' monitor (`#crystools-root-easyuse`), and **Crystools is not installed**.
- **Native version:** handle one WS event and draw a top-bar box with bars and sparklines. No client-side logic is needed. **Small.**

---

## 2. LC123: LC Align and the other canvas tools

`CN/ComfyUI_LC123_nodes/web`, about 26.8K lines of JS in total.

### LC Align (`lc_align.js`, 1025 lines)
- **UI:**
  - Extension `"LC123.Align"` at :953-954.
  - A logo button in the toolbar, inserted before `app.menu.settingsGroup.element` (`mountButton` :847-880).
  - Overlays drawn on the canvas, with pointer listeners on `window` (:946-950).
- **Settings** (:22-27, :953-1012):
  - `LC123.Align.Enabled` (default true)
  - `LC123.Align.Rulers` (default true)
  - `LC123.Align.RevealDistance` (slider, default 120)
  - `LC123.Align.SnapItems` (default false)
  - Snap step reuses `Comfy.SnapToGrid.GridSize` (default 10).
- **Behaviour:**
  - 18 px rulers along the top and left edges. Left-click adds a node guide line; right-click adds a group guide line.
  - Colours: node `#38BDF8`, group `#F59E0B`, match `#A3E635`.
  - Smart guides and snapping while dragging.
- **What it saves in the workflow:**
  - `graph.extra.lc_guides = [{axis:"x"|"y", v, kind:"node"|"group"}]`
  - `graph.extra.lc_align_on`
- **Routes:** none. It is all client-side geometry. **Medium** (≈1K lines of math and drawing).

### Other LC123 canvas tools (all client-only)

| File:line | What it does | Size |
|---|---|---|
| `lc_connection_fx.js:243` "LC123.ConnectionFX" | Highlights compatible sockets while you drag a wire | S |
| `lc_recent_colors.js:196` | Remembers the last 8 custom colours in the node "Colors" context menu | S |
| `lc_pin_all.js:58` | Adds Pin/Unpin all to the node menu | XS |
| `lc_node_colors.js:138`, `lc_color.js`, `lc_folder.js` | Default node colours | S |
| `lc_update_nodes.js:265` | Command that rebuilds outdated LC nodes | S |
| `lc_optimizer.js:727` | Canvas performance report, opened from the canvas menu; uses `GET /lc123/optimizer/scan` | M |
| `lc_performance_settings.js:114`, `lc_vue_zoom.js`, `lc_graph_events.js` | Perf settings, zoom, event bridge | — |

### Virtual nodes (exist only in the frontend)
These are **required**: workflows that use them break without a native version.
- `lc_bypasser.js`: "LC Bypasser" :559, "LC Mute" :584, "LC Bypasser Panel" :903 (`isVirtualNode` :215, :594).
- `lc_groups_bypasser.js`: "LC Groups Bypasser" :676 (:256).
- `lc_note.js`: "LCNote" :574.
  - Markdown note; translations stored in `properties.lc_note`.
  - Optional calls: `/lc_vision/translate`, `GET /lc123/link_card?url=`.

### Widgets on LC123's own backend nodes
- Many extensions of this kind: slider, sigmas, image preview, label, control panel, regional canvas.
- `lc_lora_loader` calls `GET /lc123/lora_loader/info?lora=` to get trigger words and base model.
- The regional canvas calls `POST /anima/canvas/apply`.

### Other LC123 routes
`/lc123/model_info` (POST), `/lc123/sigmas/preview|save` (POST), `/lc123/last_image_holder/clear`, `/lc123/image_label/get_image`, `/lc/pack_versions`, `/lc123/packusage/run`, `/lc123/optimizer_node/choices?kind=&base=`, `/lc123/optimizer_node/plan` (POST), `/lc123/rtx/status|install`, `/lc123/sounds/{filename}`.

Syscheck routes (`lc_system_check.py:72-125`):
- `GET /lc123/syscheck/run`: returns 409 if the queue is busy; saves `user/LC123/system_profile.json`.
- `GET /lc123/syscheck/profile`: 404 if no profile exists.
- `GET /lc123/syscheck/models` → `[{id,name,kind,sizes[]}]`.
- `GET /lc123/syscheck/recommend?model=&goal=Optimal&mp=`.

**Size:** Align medium; the virtual bypasser/note nodes medium; the small tools S each.

---

## 3. rgthree-comfy

### Routes
- `py/server/rgthree_server.py`:
  - `GET /rgthree/api/print?type=` (:28)
  - `GET /rgthree/api/incompatible-extensions` (:38)
- `py/server/routes_config.py`:
  - `GET /rgthree/config.js` (:14)
  - `GET /rgthree/api/config[?refresh]` (:22)
  - `POST /rgthree/api/config`, form field `json` (:30)
  - `/rgthree/logo.svg` (:39), `/rgthree/logo_markup.svg` (:69)
- `py/server/routes_model_info.py`, where `{type}` is `loras` or `checkpoints`:
  - `GET /rgthree/api/{type}[?format=details]`: list of filenames, or file-info objects with `format=details` (:23)
  - `GET .../info?files=&light=` (:67)
  - `GET .../info/refresh` (:87)
  - `GET .../info/clear?files=ALL|csv&del_info&del_metadata&del_civitai` (:100)
  - `POST .../info?file=` (:132)
  - `GET .../img` (:152)
- Its own web pages: `/rgthree/link_fixer/` (`web/link_fixer/index.html`) and `/rgthree/models/` (only in some setups).
- **No custom WS events.** The progress bar uses the standard events `status / execution_start / executing / progress / execution_cached / executed / execution_error` (`web/common/prompt_service.js:108-158`).

### Features

| Feature | Registration | Pure frontend? |
|---|---|---|
| Fast Groups Muter / Bypasser | `fast_groups_muter.js:429`, `fast_groups_bypasser.js:18` | ✅ virtual node |
| Fast Muter / Bypasser, Mute/Bypass Relay & Repeater, Fast Actions Button (:269), Random Unmuter (:93), Node Collector (:93) | base class `base_node.js:235` | ✅ virtual nodes |
| Reroute | `reroute.js:981` (`isVirtualNode` :252) | ✅ virtual node |
| Bookmark (with keyboard shortcuts) | `bookmark.js:102` | ✅ virtual node |
| Label | `label.js:160` | ✅ virtual node |
| Context / Big / Switch / Merge, Any Switch, Display Any/Int, Power Prompt(+Simple), SDXL nodes, KSampler Config, Image Comparer, Image Inset Crop, Image Resize, Power Primitive, Power Puter, Image or Latent Size | backend nodes (`__init__.py:47-76`); JS only adds dynamic inputs and drawing | ⚠️ need the server; UI tweaks are client-side |
| Seed (−1 means random) | `seed.js:203` | ✅ the random value is chosen client-side before queueing — a native client must do the same |
| Power Lora Loader | `power_lora_loader.js:621` | ⚠️ widget needs `/rgthree/api/loras?format=details`; info dialog needs `/info` |
| Progress bar | `rgthree.js:208-266` | ✅ standard WS events |
| Top-bar menu: settings dialog + bookmarks list | `comfy_ui_bar.js:86` "rgthree.TopMenu" (inserted before `settingsGroup` :18, :84); `config.js:245` RgthreeConfigDialog | ⚠️ settings stored via `/rgthree/api/config` |
| Canvas-menu "rgthree" submenu, queue selected output nodes | `rgthree.js:776`, `menu_queue_node.js:45` | ✅ |
| Fast toggle on group headers | `feature_group_fast_toggle.js:29` | ✅ |
| Context-menu auto-nest, copy image | `menu_auto_nest.js:6`, `menu_copy_image.js:23` | ✅ |
| Link fixer | `/rgthree/link_fixer/` page | ✅ (separate page) |

Config keys: `features.progress_bar.enabled/position/height`, `features.import_individual_nodes.enabled`, `features.comfy_top_bar_menu.enabled`, `features.menu_bookmarks.enabled`, `features.group_header_fast_toggle.enabled`.

**Power Lora Loader serialization:**
- Each row is a widget value `{on:true, lora:null, strength:1, strengthTwo:null}` (:353-356), sent to the backend as kwargs `lora_N`.
- The backend (`py/power_lora_loader.py:38-58`) reads any key starting with `LORA_`; `strengthTwo` is the CLIP strength.

**Size:** large overall.
- Fast Groups Muter/Bypasser, Reroute, Bookmark, Label, Node Collector and the relays/repeaters are medium as a set. Workflows break without them.
- Power Lora Loader: medium.
- Progress bar: small.
- Settings dialog: small.

---

## 4. LoRA Manager (`CN/comfyui-lora-manager`)

### Its own web app (served by the plugin; open in a browser or embed with WebKitGTK)
- `GET /loras`, `/checkpoints`, `/embeddings` (`py/routes/model_route_registrar.py:146`)
- `GET /loras/recipes` (`recipe_route_registrar.py:21`)
- `GET /statistics` (`stats_routes.py:632`)
- Static files: `/loras_static`, `/locales`, `/example_images_static` (`py/lora_manager.py:148-161`)
- Progress WebSockets: `/ws/fetch-progress`, `/ws/download-progress`, `/ws/init-progress` (`py/lora_manager.py:180-184`)
- `standalone.py` also lets it run as a separate server.

### REST API (`/api/lm/...`)
- **Per model type** `{prefix}` = `loras|checkpoints|embeddings` (`model_route_registrar.py:24-145`): `list`, excluded/exclude/unexclude, delete, bulk-delete, fetch-civitai, fetch-all-civitai, relink-civitai, replace-preview, save-metadata, add-tags, rename, move_model, move_models_bulk, auto-organize, top-tags, search-tags, base-models, model-types, scan, roots, folders, folder-tree, find-duplicates, verify-duplicates, get-notes, preview-url, civitai-url, metadata, relative-paths, cancel-task.
- **LoRA-only** (`lora_routes.py:46-73`): letter-counts, `get-trigger-words?name=`, `usage-tips-by-path?relative_path=`, `POST random-sample`, `POST cycler-list`, `POST get_trigger_words`.
- **`GET /api/lm/loras/list`:**
  - Query parameters: `page, page_size, sort_by, folder, search, fuzzy_search, tags, base_model*, tag_include*, tag_exclude*, folder_include*, folder_exclude*, model_type*, civitai_model_type*, auto_tag_include*` (`*` means it can be repeated).
  - Response: `{items:[{model_name,file_name,preview_url,preview_nsfw_level,...}], total, page, page_size, total_pages}`.
- **Downloads:** `POST /api/lm/download-model`, `download-model-get`, `cancel-download-get`, `skip-download`, `pause-download`, `resume-download`, `downloads/queue`, `downloads/queue/add`, `downloads/history`, `downloads/stats`.
- **Misc** (`misc_route_registrar.py:23-91`): `settings` GET/POST, `settings/libraries[/activate]`, `health-check`, `wildcards/search`, `custom-words/search`, `trained-words`, `update-usage-stats` (POST), `get-usage-stats`, `update-lora-code` GET/POST, `register-nodes` (POST), `update-node-widget` GET/POST, `get-registry`, `check-model(s)-exist`, `civitai/user-models`, `backup/*`, `doctor/*`, `llm/*`, `example-workflows`, `base-models[/refresh]`.
- **Stats:** `/api/lm/stats/{collection-overview, usage-analytics, model-usage-list, base-model-distribution, tag-analytics, storage-analytics, insights}`.
- **Updates:** `check-updates`, `version-info`, `perform-update`, `switch-channel`.
- **Recipes:** `/api/lm/recipes`, `recipe/{id}` (+`/share`, `/syntax`, `/repair`, `/rematch`), `recipes/{analyze-image, save, import-remote, import-from-url, batch-import/start, for-lora, scan, ...}`.
- **Example images:** `download-`, `import-`, `pause-example-images`, `example-images-status`.
- **Other:** `/api/lm/undo-delete`, `/api/lm/previews?path=`.

### What it injects into the ComfyUI frontend (`WEB_DIRECTORY = ./web/comfyui`)
- **Top-bar button** "LoraManager.TopMenu" (`top_menu_extension.js:157`):
  - Uses `actionBarButtons` on frontend ≥ 1.33.9 (:239); otherwise a button group before `settingsGroup` (:125).
  - Opens `/loras` in a new tab (`window.open`, :11-19).
- **Node widgets:**
  - The `loras` list widget (`loras_widget.js:797`, `addDOMWidget`), used on "Lora Loader / Lora Stacker / WanVideo Lora Select / Create Hook LoRA (LoraManager)".
  - Value is an array of `{name, strength, clipStrength, active, expanded, locked, selected}`. The backend accepts it wrapped as `{"__value__":[...]}` or as a bare list (`py/nodes/utils.py:97-112`).
  - Only entries with `active` are loaded; `clipStrength` falls back to `strength` (`py/nodes/lora_loader.py:52-66`).
  - The text input is a custom `AUTOCOMPLETE_TEXT_LORAS` type holding `<lora:name:strength[:clip]>` text.
  - Bundled Vue custom widgets (`vue-widgets/lora-manager-widgets.js:16670`, `getCustomWidgets`): `LORA_POOL_CONFIG`, `RANDOMIZER_CONFIG`, `CYCLER_CONFIG`.
  - Tag and trigger-word widgets: `tags_widget.js`, `trigger_word_toggle.js:178`.
- **Autocomplete in prompts** (`autocomplete.js`): uses `/api/lm/settings`, `usage-tips-by-path`, `custom-words/search`, `wildcards/search`.
- **Preview tooltip on hover** (`preview_tooltip.js:249`): calls `/{type}/preview-url?name=&license_flags=true`.
- **Usage stats** (`usage_stats.js:27`): on `execution_success` it posts usage data.
- **Node marker context menu** (`node_marker.js:154`): sets `node.properties.lm_marker_role`.
- Also: Settings page (`settings.js:396`), lora_info, save_image_extra_output, dynamic inputs for the stacker and prompt nodes.

### The workflow-registry loop
This is what makes "Send to ComfyUI" from the `/loras` page work, so a native client must implement it.

1. **Native client registers its nodes:** `POST /api/lm/register-nodes` with `{nodes:[...], client_id}` (`workflow_registry.js:395`), sent:
   - after a workflow loads;
   - when nodes are added or removed, or links change (debounced);
   - and **forced** whenever it receives WS `lora_registry_refresh {}` (`misc_handlers.py:3455`).
   
   Each node entry:
   ```json
   {"node_id","graph_id","graph_name","bgcolor","title","type","comfy_class","mode","marker_role",
    "capabilities":{"supports_lora","has_text_widget","text_widget_connected","widget_names":[...]}}
   ```
   Which nodes to include (`workflow_registry.js:8-22`):
   - the LoRA classes above;
   - nodes with a `ckpt_name` or `unet_name` widget;
   - "Prompt (LoraManager)", "Text (LoraManager)" and `CLIPTextEncode`;
   - nodes with a marker role.
2. **Native client applies server pushes:**
   - `lora_code_update {id|node_id, graph_id?, lora_code, mode:"append"|"replace"}`. `id:-1` means apply to every compatible node (`misc_handlers.py:1723`, `lora_loader.js:23-60`).
   - `lm_widget_update {id, graph_id?, value, mode, action?, widget_name?}` (`misc_handlers.py:3574/3698`).
   - `trigger_word_update {id, graph_id?, message}` (`lora_routes.py:284`).

### Native version needed
- **Minimum, small–medium:**
  - Open the `/loras` web app in a webview or browser (it is self-contained).
  - Implement the registry loop above.
  - A native editor for the `loras` list widget (toggle, strength, clip strength, ordering), so workflows work and serialize correctly.
- **Nice to have, medium:** autocomplete, preview tooltips, trigger-word widgets, the Pool/Randomizer/Cycler widgets.
- **Rewriting the manager pages natively: large.** Not worth it; embed them instead.

---

## 5. Other custom nodes that register sidebar or bottom-panel tabs

| Pack | Registration | Tab | Backend |
|---|---|---|---|
| midfr-comfyui-nodes (resource_reaper) | `A-toolbox/ComfyUI-Cleaner/web/resource_reaper.js:398-407` "ResourceReaper.Panel" | Sidebar **"Memory"** (polls on a timer, :388) | `api.py:22` GET `/reaper/status`, :30 POST `/reaper/clean`, :42/:46 GET/POST `/reaper/config`, :54 POST `/reaper/terminate` |
| comfy_mtb | `web/mtb_input_output_sidebar.js:251` ext `mtb.io-sidebar`, :409 | Sidebar **"Input & Outputs"** | listens to the `status` event (:498) |
| ComfyUI-EasyIllustrious | `web/js/illustrious_ui.js:93`, :144 | Bottom panel **"Illustrious Help"** | — |
| comfyui-easy-use | `web_version/v2/easyuse.js` (minified) | registers a sidebar tab (I didn't extract its title from the minified bundle) | — |

The VideoHelperSuite `testRunner.js` match is test code only, so it's excluded.

---

## Recommended priority order for native support

1. **Generic virtual-node runtime plus rgthree virtual nodes** (Reroute, Fast Groups Muter/Bypasser, Bookmark, Label, Node Collector, relays/repeaters) and **LC123's virtual nodes** (LC Bypasser/Mute/Panel, LC Groups Bypasser, LCNote). Saved workflows depend on them and break or run wrongly without them. The rgthree Seed `-1` handling belongs here too, because it happens client-side before queueing.
2. **LoRA Manager's `loras` widget and the Power Lora Loader widget.** These are the user's main loader nodes; they need the exact value formats above and `/api/lm/loras/list` plus `/rgthree/api/loras` for pickers.
3. **LoRA Manager integration:** a top-bar button that opens `/loras` in a WebKitGTK view, plus the registry loop. **Small.**
4. **DaSiWa system monitor** in the native header bar, fed by `dasiwa.system_monitor`. **Small, very visible.**
5. **LC Align** guides, rulers and snapping, keeping `graph.extra.lc_guides` and `graph.extra.lc_align_on` compatible. **Medium, all client-side.**
6. **rgthree extras:** progress bar (standard WS events), group-header fast toggle, settings dialog via `/rgthree/api/config`, bookmarks menu, queue selected outputs.
7. **Smaller LC123 tools** (connection FX, recent colours, pin all, optimizer report) and LoRA Manager's autocomplete, tooltips and trigger-word widgets.
8. **Other packs' sidebars** (resource_reaper "Memory", mtb "Input & Outputs") only if the user asks. Their APIs are small and listed above.

**Fallback for everything else:** a "Open in web UI" escape hatch (embed the stock frontend for the rare plugin), rather than reimplementing every widget.