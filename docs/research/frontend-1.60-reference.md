Now extension registration in extensionService and bottom panel/extensionManager.
📦 [ACP] Compressed m00002–m00058, m00065–m00066 → 2 block(s), ~50296 tokens saved. Excluded 1 protected message(s) m00064 from compression range (recent/last-user zone) — they stay visible outside the new block; do not target them in another compress call. Last-user anchor: m00001 "Read-only research. Source: ComfyUI fron".
Now ExecutableNodeDTO.Now WebSocket handling.Now the WS payload schemas.# ComfyUI Frontend 1.60.0: Reference for the Rust/GTK4 Port

Paths are relative to `/home/montel/Documents/ComfyUI_frontend-1.60.0`.

---

## 1. Sidebar, bottom panel, top menu and actionbar

### Registration
- **Core tabs** are registered in `src/composables/sidebarTabs/registerCoreSidebarTabs.ts:13-60`.
- **Store:** `src/stores/workspace/sidebarTabStore.ts:23` has `registerSidebarTab(tab, {prepend})`.
  - It also registers a command `Workspace.ToggleSidebarTab.${tab.id}` (`:59-73`).
  - `unregisterSidebarTab` is at `:76`. It calls `destroy()` for custom tabs.
- **Deprecated proxy:** `src/stores/workspace/workspaceStore.ts:124`.

### Sidebar tabs

| id | icon | title key | Definition | Backend endpoints |
|---|---|---|---|---|
| `job-history` (prepended; only when setting `Comfy.Queue.QPOV2` is on, `:16-34`) | `icon-[lucide--history]` | `queue.jobHistory` | `useJobHistorySidebarTab.ts:10` | `GET /jobs?status=&limit=&offset=` (`src/platform/remote/comfyui/jobs/fetchJobs.ts:62`), `/jobs/{id}` (`:177`), `/jobs/{id}/assets` (`:218`) |
| `assets` | `icon-[comfy--image-ai-edit]` | `sideToolbar.assets` | `useAssetsSidebarTab.ts:10` → `AssetsSidebarTab.vue` | outputs come from `/jobs`; inputs from `GET /internal/files/input` (`src/stores/assetsStore.ts:46`); thumbnails from `/view?…`; cloud uses `/assets` (`assetService.ts:190`) |
| `node-library` | `icon-[comfy--node]` | — | `useNodeLibrarySidebarTab.ts:17` (V1/V2 components) | `GET /object_info`; bookmarks are stored in settings |
| `model-library` | `icon-[comfy--ai-model]` | — | `useModelLibrarySidebarTab.ts:14` | `GET /experiment/models` (`src/stores/modelStore.ts:356`), `/experiment/models/{folder}` (`:317`), `/view_metadata/{folder}?filename=` (`:113`) |
| `workflows` | `icon-[comfy--workflow]` | — | `src/platform/workflow/management/composables/useWorkflowsSidebarTab.ts:12` | `GET /userdata?dir=workflows&recurse=true&split=false&full_info=true` (`workflowStore.ts:422`); files live under userdata `workflows/`; subgraph blueprints under `subgraphs/` (`src/stores/subgraphStore.ts:45,249`) |
| `apps` | `icon-[lucide--panels-top-left]` | `linearMode.appModeToolbar.apps` | `useAppsSidebarTab.ts:8` | — |

**Templates** are a dialog, not a tab (command `Comfy.BrowseTemplates`):
- `GET /workflow_templates` (`api.ts:1268`)
- `/templates/{file}` (`api.ts:1282`)
- `/workflow_templates/{module}/{name}.json` (`useTemplateWorkflows.ts:435`)

### Bottom panel
Registered in `src/composables/bottomPanelTabs/registerCoreBottomPanelTabs.ts:6-28`:
- `shortcuts-essentials` and `shortcuts-view-controls` (`useShortcutsTab.ts:12,20`, `targetPanel: 'shortcuts'`).
- `logs-terminal`, title "Logs" (`useTerminalTabs.ts:9`). Not on cloud.
  - Endpoints: `/internal/logs`, `/internal/logs/raw`, `/internal/logs/subscribe` (`api.ts:1878-1895`), plus the WS `logs` message.
- `command-terminal`: desktop builds only (`useTerminalTabs.ts:19`).
- Extension tabs are added by `bottomPanelStore.ts:129` `registerExtensionBottomPanelTabs`.

### Actionbar and top menu
- **Queue button:** `src/components/actionbar/ComfyRunButton/ComfyQueueButton.vue`.
  - Three modes: **Run**, **Run (On Change)**, **Run (Instant)** (`:111-138`).
  - Shift+click runs `Comfy.QueuePromptFront`; a normal click runs `Comfy.QueuePrompt` (`:254-269`).
- **Queue settings:** `src/stores/queueSettingsStore.ts:3`.
  - `AutoQueueMode = 'disabled'|'change'|'instant-idle'|'instant-running'`.
  - State is `{mode, batchCount: 1}`. The batch count UI is `BatchCountEdit.vue`.
- **Queue commands:** `src/composables/useCoreCommands.ts:527` `Comfy.QueuePrompt` → `app.queuePrompt(0, batchCount)`. `:547` `QueuePromptFront` uses `-1`.
- **Interrupt and clear:** `ComfyActionbar.vue:413` `Comfy.Interrupt {jobId}`, `:427` `Comfy.ClearPendingTasks`.
- **Selection toolbox:** its Execute button runs `Comfy.QueueSelectedOutputNodes`, which uses `partial_execution_targets`.
- **Menu bar:** `src/constants/coreMenuCommands.ts:3` `CORE_MENU_COMMANDS`:
  - New
  - File: Open, Save, SaveAs, Export, ExportAPI
  - Edit: Undo/Redo, Copy/Paste/SelectAll, ClearWorkflow, OpenClipspace, RefreshNodeDefinitions, `Memory.UnloadModels` (and `…AndExecutionCache`)
  - View: `Workspace.ToggleBottomPanel`, `Comfy.BrowseTemplates`, `Workspace.ToggleFocusMode`, `Comfy.ToggleCanvasInfo`, `Comfy.Canvas.ToggleMinimap`, `Comfy.Canvas.ToggleLinkVisibility`, Zoom In/Out, Fit View
  - Help
- **Topbar components:** `src/components/topbar/` (WorkflowTabs, TopbarBadges, ActionBarButtons).

---

## 2. Extension API

### `ComfyExtension` (`src/types/comfy.ts:292`)

**Declarative fields:**
- `name`, `provenance?`
- `commands?: ComfyCommand[]`
- `keybindings?: Keybinding[]`
- `menuCommands?: {path: string[]; commands: string[]}[]`
- `settings?: SettingParams[]`
- `bottomPanelTabs?`, `aboutPageBadges?`, `topbarBadges?`, `actionBarButtons?`
- An index signature `[key: string]: unknown`

**Hooks** (line numbers in `comfy.ts`):

| Hook | Line |
|---|---|
| `init(app)` | `:333` |
| `setup(app)` | `:337` |
| `addCustomNodeDefs(defs, app)` | `:342` |
| `getCustomWidgets(app) → Record<string, CustomComfyWidgetConstructor>` | `:352` |
| `getSelectionToolboxCommands(item) → string[]` | `:359` |
| `getCanvasMenuItems(canvas)` | `:366` |
| `getNodeMenuItems(node)` | `:373` |
| `beforeRegisterNodeDef(nodeType, nodeData, app)` | `:381` |
| `beforeRegisterVueAppNodeDefs(defs, app)` | `:394` |
| `registerCustomNodes(app)` | `:400` |
| `loadedGraphNode(node, app)` | `:408` |
| `nodeCreated(node, app)` | `:414` |
| `beforeLoadGraph` | `:416` |
| `afterLoadGraph` | `:418` |
| `onGraphLoadError(error, app)` | `:425` |
| `beforeConfigureGraph(graphData, missingNodeTypes, app)` | `:433` |
| `afterConfigureGraph(missing, app)` | `:444` |
| `onAuthUserResolved(user, app)` | `:454` |
| `onAuthTokenRefreshed` | `:460` |
| `onAuthUserLogout` | `:466` |
| `onNodeOutputsUpdated(nodeOutputs)` | `:468` |

### Supporting types in `src/types/comfy.ts`
- `:48` `ComfyWidgetConstructor = (node, inputName, inputData: InputSpec, app, widgetName?) => {widget, minWidth?, minHeight?}`
- `:223` `AboutPageBadge {label, url, icon, severity?: 'danger'|'warn'}`
- `:242` `TopbarBadge {text, label?, variant?: 'info'|'warning'|'error', icon?, tooltip?}`
- `:269` `ActionBarButton {icon, label?, tooltip?, class?, onClick}`
- `:99` `ComfyApp` exposes `api`, `ui`, `extensionManager`, `graph`, `rootGraph`, `canvas`, `graphToPrompt(graph?) → {workflow, output}`, `queuePrompt(number, batchCount?, options?)`, `registerExtension`, `loadGraphData`, and more.

### `src/types/extensionTypes.ts`
- `:19` `BaseSidebarTabExtension {id, title, icon?: string|Component, iconBadge?, tooltip?, label?, onToggle?}`
- `:29` `BaseBottomPanelExtension {id, title?, titleKey?, targetPanel?: 'terminal'|'shortcuts'}`
- `VueExtension {id, type: 'vue', component}`
- `:42` `CustomExtension {id, type: 'custom', render(container: HTMLElement), destroy?}`. This is the path third-party extensions use for plain-DOM tabs.
- `:66` `ToastMessageOptions {closable?, detail?, life?, severity?, summary?}`
- `:74` `ToastManager {add (deprecated), addAlert, dismiss, dismissAll, error/info/loading/success/warning(title, options?) → ToastId}`
- `:88` `ExtensionManager`:
  - `registerSidebarTab`, `unregisterSidebarTab`, `getSidebarTabs`
  - `toast`, `dialog`
  - `command: {commands, execute(id, {errorHandler?, metadata?})}`
  - `setting: {get, set}`
  - `workflow`, `lastNodeErrors`, `lastExecutionError`
  - `renderMarkdownToHtml(md, baseUrl?)`

### Commands, keybindings, settings
- **Commands:** `src/stores/commandStore.ts:10` `ComfyCommand {id, function(metadata?), label?, icon?, tooltip?, menubarLabel?, versionAdded?, confirmation?, source?, active?, category?: 'essentials'|'view-controls', mutatesGraph?}`
- **Keybindings:** `src/platform/keybindings/types.ts` defines `KeyCombo {key, ctrl?, alt?, shift?, meta?}` and `Keybinding {commandId, combo, targetElementId?}`.
- **Settings:** `src/platform/settings/types.ts`
  - `:222` `SettingParams extends FormItem {id, defaultValue, defaultsByInstallVersion?, onChange?(new, old), category?: string[], experimental?, deprecated?, migrateDeprecatedValue?, versionAdded?, versionModified?, sortOrder?, hideInVueNodes?}`
  - `:251` `FormItem {name, type, tooltip?, attrs?, options?}`
  - `:186` setting types: `boolean | number | slider | knob | combo | radio | text | image | color | url | hidden | backgroundImage`, or a custom renderer function.
  - The legacy `app.ui.settings.addSetting` is at `src/scripts/ui/settings.ts:115`.

### Loading order (`src/services/extensionService.ts`)
- `:93` `registerExtension` holds extensions in a queue. `:102` `registerExtensionNow` processes them:
  - keybindings (`:110`)
  - commands
  - menu commands (`menuItemStore.ts:95`)
  - settings (`:115`)
  - bottom panel tabs
  - `getCustomWidgets` into `widgetStore`
- Hooks are dispatched by `invokeExtensions` (`:215`) and `invokeExtensionsAsync` (`:252`).
- Badges and buttons are read by `aboutPanelStore.ts:73`, `topbarBadgeStore.ts:12` and `actionBarButtonStore.ts:12`.

---

## 3. Subgraphs

### Schema
Zod schemas are in `src/platform/workflow/validation/schemas/workflowSchema.ts`:

| Schema | Line | Fields |
|---|---|---|
| `zComfyWorkflow` (format 0.4) | `:422` | `{last_node_id, last_link_id, nodes, links: tuple[], floatingLinks?, groups?, config?, extra?, version: number, models?, definitions?}` |
| `zComfyWorkflow1` | `:474` | `{version: literal(1), state: {lastGroupId, lastNodeId, lastLinkId, lastRerouteId} (:80), groups, nodes, links: object[], floatingLinks, reroutes, extra, models, definitions: {subgraphs}}` |
| `zGraphDefinitions` | `:408` | `{subgraphs: SubgraphDefinition[]}` |
| `zSubgraphDefinition` | `:554` | Workflow1 + `{id: uuid, revision, name, description?, category?, essentials_category?, inputNode, outputNode, inputs?, outputs?, widgets?, definitions?}` |
| `zExportedSubgraphIONode` | `:503` | `{id, bounding: [x, y, w, h], pinned?}` |
| `zSubgraphIO` | `:319` | `zNodeInput` + `{id: uuid, type: string, linkIds?}` |
| `zExposedWidget` | `:509` | `{id: nodeId, name}` |
| `zSubgraphInstance` | `:328` | a node whose `type` is a uuid |

**Links:**
- 0.4 tuple (`:89`): `[id, origin_id, origin_slot, target_id, target_slot, type]`
- v1 object (`:113`): `{id, origin_id, origin_slot, target_id, target_slot, type, parentId?, hidden?, label?}`

**Reroutes** (`:126`): `{id, parentId?, pos, linkIds, floating?: {slotType}}`.

**Nodes and groups:**
- `zComfyNode` (`:301`): `{id, type, pos, size, flags, order, mode, inputs?, outputs?, properties, widgets_values?, color?, bgcolor?}`
- `zNodeInput` (`:149`): `{name, type, link?, slot_index?}`
- `zNodeOutput` (`:140`): `{name, type, links?, slot_index?}`
- `zGroup`: `:345`. `zExtra` (`:370`) holds `ds`, `frontendVersion`, `linkExtensions`, `reroutes`, `linearData`, and more.

`validateComfyWorkflow` (`:724`) uses the v1 schema when `version === 1`, otherwise 0.4.

### Version numbers
- `LiteGraph.VERSION = 0.4` (`src/lib/litegraph/src/LiteGraphGlobal.ts:76`).
- `LGraph.serialisedSchemaVersion = 1` (`src/lib/litegraph/src/LGraph.ts:526`).
- **What actually gets saved:**
  - `LGraph.serialize()` (`LGraph.ts:2948-3000`) writes the 0.4 shape: tuple links and `version: 0.4`.
  - When reroutes exist, it adds `extra.reroutes` and `extra.linkExtensions [{id, parentId}]`, and it can add `extra.linkPresentation`.
  - Each entry in `definitions.subgraphs` comes from `Subgraph.asSerialisable()` (`LGraph.ts:3788-3810`). These use the **v1 shape**: `version: 1`, `state`, object links.
  - So one file mixes a 0.4 root with v1 subgraph definitions.

### Where definitions live
- They are kept **flat at the root** in `definitions.subgraphs`.
- `asSerialisable` (`LGraph.ts:3051-3058`) only writes definitions on the root graph. It writes every subgraph that is used, including nested ones, which it finds transitively with `findUsedSubgraphIds` (`subgraphUtils.ts:499`).
- On load, definitions are configured in dependency order via `topologicalSortSubgraphs` (`LGraph.ts:2197`).

### Fields of a definition (`ExportedSubgraph`)
Defined in `src/lib/litegraph/src/types/serialisation.ts:160-199`:
- `name`, `category?`, `description?`
- `inputNode` and `outputNode`: `{id, bounding, pinned?}`
- `inputs?` / `outputs?`: `SubgraphIO {id: UUID, type: string, linkIds?: number[]}` plus slot fields such as `name`, `label` and `localized_name`
- `widgets?: ExposedWidget[] {id, name}`
- Everything from the graph body: `nodes`, `links`, `groups`, `reroutes`, `state`, `config`, `extra`

**Inner links to the subgraph boundary** use sentinel node ids:
- `SUBGRAPH_INPUT_ID = -10` and `SUBGRAPH_OUTPUT_ID = -20` (`src/lib/litegraph/src/constants.ts:10,13`).
- `LLink.originIsIoNode` means `origin_id === -10`; `targetIsIoNode` means `target_id === -20` (`LLink.ts:257-263`).
- The `origin_slot` of a link from `-10` is the index into `subgraph.inputs`.

### How an instance references its definition
- An instance is a normal node with **`type` set to the subgraph UUID** (`ExportedSubgraphInstance`, `serialisation.ts:131-136`).
- Each definition is registered as a node type under its id: `LiteGraph.registerNodeType(subgraph.id, node)` (`src/services/litegraphService.ts:546`, inside `registerSubgraphNodeDef` at `:432`).
- `src/services/subgraphService.ts:42-60` adds a synthetic node def: `{name: id, display_name: name, category: 'subgraph', input: {required: {}}, output: [] …}`.
- The `SubgraphNode` constructor is `super(subgraph.name, subgraph.id)` (`SubgraphNode.ts:130-135`).
- So `LiteGraph.createNode(n_info.type)` (`LGraph.ts:3291`) creates instances the same way as any other node.

### Promoted widgets in 1.60
- **How a widget gets promoted:**
  - The host `SubgraphNode` has an input for each `subgraph.inputs[i]`.
  - If that subgraph input is linked inside to a widget input of an interior node, the host input gets a widget that copies the interior widget's `type`, `options`, `value`, `serialize` and `disabled`.
  - The copy is stored in `useWidgetValueStore` under `widgetId(rootGraph.id, hostNode.id, subgraphInput.name)` (`SubgraphNode.ts:690-712` `_setWidget`).
  - Nested promotion (an interior node that is itself a SubgraphNode) is resolved by `_resolveNestedPromotedSource` (`:654`).
- **What the host's widgets are:** `SubgraphNode.widgets` is a derived getter: promoted widgets per input, followed by `_extraWidgets` (`:138-146`).
- **Saving and loading:** the host's `widgets_values` is **one value per input that has a widget, in input order** (`serializeFromStoreState`, `:1018-1056`). `_applyPromotedWidgetValues` (`:501-516`) restores them by the same index.
  - The host `properties` may also hold `previewExposures` and `proxyWidgetErrorQuarantine`.
- **Legacy format:**
  - The old format was `properties.proxyWidgets = [[sourceNodeId, widgetName, disambiguator?]]` (`src/core/schemas/promotionSchema.ts:7-10`).
  - On load, `src/core/graph/subgraph/migration/proxyWidgetMigration.ts:174` `flushProxyWidgetMigration` turns these entries into real subgraph inputs, then deletes `proxyWidgets` (`:254`).
  - `LGraph.ts:3414` warns if they were not migrated.
- **Legacy `widgets: ExposedWidget[]`** is still serialized on the definition (`LGraph.ts:3804`).

---

## 4. graphToPrompt (`src/utils/executionUtil.ts:59-207`)

### Steps
1. **Apply virtual nodes (`:65-74`).** For every node in `computeExecutionOrder`, expanded through `getInnerNodes`, call `innerNode.applyToGraph()` on virtual nodes.
   - PrimitiveNode copies its first widget's value into the target nodes' widgets (`src/extensions/core/widgetInputs.ts:58-68`, `applyFirstWidgetValueToGraph`).
2. **Build the workflow JSON (`:76-90`).** Call `graph.serialize({sortNodes})`, delete `localized_name` from every slot, run `compressWidgetInputSlots(workflow)` (drops unconnected widget-input slots), and set `extra.frontendVersion`.
3. **Flatten into DTOs (`:92-112`).** Each top-level node becomes an `ExecutableNodeDTO(node, [], map)`.
   - Nodes with mode NEVER (2) or BYPASS (4) are skipped before expansion, so a muted or bypassed subgraph is never expanded.
   - Other nodes contribute `dto.getInnerNodes()`, which runs `SubgraphNode.getInnerNodes` (`SubgraphNode.ts:882-930`) recursively. It throws `RecursionError` on cycles.
4. **Execution ids.** `this._id = [...subgraphNodePath, node.id].join(':')` (`ExecutableNodeDTO.ts:121`).
   - Inner node 5 of host 12 becomes **`"12:5"`**. Deeper nesting gives `"12:7:5"`.
   - The SubgraphNode itself also gets a DTO (`SubgraphNode.ts:904-910`), used only to resolve inputs. It is virtual (`isVirtualNode = true`, `:95`), so it never appears in the output.
5. **Emit nodes (`:116-191`).** Skip `isVirtualNode`, NEVER and BYPASS nodes. For each remaining node:
   - **Widgets:** for each widget with a `name` and `options.serialize !== false`, set `inputs[widget.name] = widget.serializeValue?.(node, i) ?? widget.value`.
     - Array values are wrapped as `{__value__: arr}`.
     - Curve widgets become `{__type__: 'CURVE', __value__}`.
   - **Inputs:** for each input, `resolveInput(i)`. A link gives `[origin_id, origin_slot]`; a resolved promoted widget gives its value (arrays wrapped). This overrides the widget value.
   - The node is emitted as `output[execId] = {inputs, class_type: node.comfyClass, _meta: {title, cnr_id?, aux_id?, ver?}}`.
6. **Prune (`:193-204`).** Delete any `[id, slot]` input whose `id` is not in the output.

### Link resolution (`src/lib/litegraph/src/subgraph/ExecutableNodeDTO.ts`)
- **`resolveInput` (`:150-252`):**
  - No link: returns `undefined`.
  - Link from the `-10` input node: moves to the host SubgraphNode's input `link.origin_slot`.
    - If the host input is unlinked, it returns `{origin_slot: -1, widgetInfo: {value}}` from the widget store (`:199-212`), which is how promoted widget values reach the prompt.
    - Otherwise it follows the outer link through the host DTO (`:232`).
  - Normal link: `outputNodeDto.resolveOutput(link.origin_slot, type, visited)`.
- **`resolveOutput` (`:264-350`):**
  - **NEVER (mute):** returns `undefined`, so the input is dropped.
  - **BYPASS:** `_getBypassSlotIndex` (`:359-392`) picks a pass-through input in this order:
    1. If the type is `*` or empty: the input with the same slot index, falling back to 0.
    2. The input with the same index, if its type is compatible.
    3. The first input with an exact type match.
    4. The first compatible input.
    It then calls `resolveInput` on it, so bypass chains are followed.
  - **SubgraphNode:** `_resolveSubgraphOutput` (`:396-430`) follows the inner link to the `-20` output node, looks up DTO `"path:host:inner"` and recurses.
  - **Virtual node (e.g. Reroute):** uses `resolveVirtualOutput` (Set/Get-style) or `getInputLink(slot)` and passes through. If neither applies it returns `undefined`.
  - Otherwise returns `{origin_id: this.id, origin_slot: slot}`.

### Virtual nodes (`isVirtualNode = true`)
- PrimitiveNode: `widgetInputs.ts:51`
- Note and MarkdownNote: `noteNode.ts:38,75`
- Legacy RerouteNode: `rerouteNode.ts:39`
- SubgraphNode: `SubgraphNode.ts:95`
- Native link reroutes (`extra.reroutes`, `link.parentId`) only change how links are drawn. They do not affect the prompt.

### Mode values
`LGraphEventMode` (`src/lib/litegraph/src/types/globalEnums.ts:83`): `ALWAYS=0, ON_EVENT=1, NEVER=2 (mute), ON_TRIGGER=3, BYPASS=4`.

### `POST /prompt` body (`api.ts:1344-1375`)
```
{ client_id, prompt: output, partial_execution_targets?,
  extra_data: { auth_token_comfy_org, api_key_comfy_org,
                comfy_usage_source: 'comfyui-frontend',
                extra_pnginfo: { workflow }, preview_method? },
  front: true  (if number === -1) | number (if number !== 0) }
```
A non-200 response raises `PromptExecutionError`.

The API node shape is `zNodeData` (`workflowSchema.ts:752-771`): `{inputs: Record<string, any | [nodeId, slot]>, class_type, _meta?: {title, cnr_id?, aux_id?, ver?}}`.

---

## 5. WebSocket (`src/scripts/api.ts:970-1205`)

### Connection
- **URL:** `ws(s)://{host}{base}/ws?clientId={window.name}`. Cloud adds auth parameters. `binaryType = 'arraybuffer'` (`:994-997`).
- **On open:** the client sends `{type: 'feature_flags', data: clientFeatureFlags}` (`:1007-1013`). The flags come from `src/config/clientFeatureFlags.json`: `supports_preview_metadata`, `supports_manager_v4_ui` and `supports_progress_text_metadata`, all true.
- **On close:** reconnects after 300 ms and dispatches `status(null)` and `reconnecting`.
  - If the socket errors before it ever opened, the client falls back to polling with `_pollQueue` (`GET /prompt`).

### Binary frames
Every frame starts with a big-endian `u32` type (`:1046-1125`).

| Type | Name | Layout |
|---|---|---|
| **1** | preview | `u32 imageType` (2 = PNG, otherwise JPEG), then the image bytes. Dispatched as `b_preview` with a Blob. |
| **3** | progress_text | Bytes after the type: if the server supports `supports_progress_text_metadata`, `u32 len + prompt_id`; then `u32 len + nodeId`; then UTF-8 text. Dispatched as `progress_text {nodeId, text, prompt_id?}`. |
| **4** | preview with metadata | `u32 metaLen`, then JSON `{image_type (mime), node_id, display_node_id, parent_node_id, real_node_id, prompt_id}`, then the image bytes. Dispatched as `b_preview_with_metadata` and also as `b_preview`. |

### JSON messages
Messages have the shape `{type, data}`. The switch is at `:1132-1197` and the payload types are in `src/platform/remote/comfyui/execution/types.ts`.

| type | Payload | Handling |
|---|---|---|
| `status` | `{status?: {exec_info: {queue_remaining}} \| null, sid?}` (`:38`) | `sid` becomes `clientId`, is written to `window.name` and to storage (`:1133-1143`) |
| `executing` | `{node: id \| null, display_node?, prompt_id}` (`:62`) | dispatched as `display_node \|\| node` |
| `executed` | `{node, display_node, prompt_id, output: {images?, audio?, video?, animated?, text?, …passthrough}, merge?}` (`:67`) | `ResultItem {filename, subfolder, type, display_name?, id?, preview_id?}` (`:9`) |
| `progress` | `{value, max, prompt_id, node}` (`:42`) | — |
| `progress_state` | `{prompt_id, nodes: Record<id, {value, max, state: 'pending'\|'running'\|'finished'\|'error', node_id, prompt_id, display_node_id?, parent_node_id?, real_node_id?}>}` (`:48-61`) | — |
| `execution_start` / `execution_success` | `{prompt_id, timestamp}` | — |
| `execution_cached` | `+ nodes: id[]` | — |
| `execution_interrupted` | `+ node_id, node_type, executed[]` | — |
| `execution_error` | `+ node_id?, node_type, executed[], exception_message, exception_type, traceback: string[], current_inputs?, current_outputs?` (`:87`) | — |
| `logs` | `{size?: {cols, row}, entries: [{t, m}]}` | — |
| `notification` | `{value, id?}` | — |
| `feature_flags` | `Record<string, unknown>` | stored as `serverFeatureFlags` |
| `agent_status` | `{state: 'starting'\|'ready'\|'failed', reason?}` | — |
| `graphChanged`, `autoQueueGraphChanged`, `promptQueued` | — | frontend and auto-queue triggers |
| `asset_download`, `asset_export`, `assets.seed.completed` | see `types.ts:128-157` | — |
| other | — | forwarded only if an extension registered for it via `api.addEventListener`; otherwise logged as "Unknown message type" |

### Mapping ids back to the canvas
- WS node ids are execution ids such as `"12:5"`.
- They are mapped back to a canvas node with `executionIdToNodeLocatorId` (`src/utils/graphTraversalUtil.ts:697`) and `getNodeByExecutionId` (`:432`).

---

## 6. REST endpoints (`src/scripts/api.ts`)

### URL helpers
- `apiURL(r)` = base + `/api` + r (`:590`)
- `internalURL(r)` = base + `/internal` + r (`:586`)
- `fileURL` (`:603`)
- `fetchApi` (`:666`) adds the `/api` prefix and auth headers.

### Endpoints in `api.ts`

| Method & path | Line | Purpose |
|---|---|---|
| GET `/prompt` | `:935` | queue status (polling fallback) |
| POST `/prompt` | `:1375` | queue a prompt (body in §4) |
| GET `/extensions` | `:1257` | list of extension JS URLs |
| GET `/workflow_templates`; `/templates/{f}` | `:1268`, `:1282` | templates |
| GET `/embeddings` | `:1303` | embeddings list |
| GET `/object_info` | `:1318/:1329` | node definitions |
| POST `/assets/from-workflow` | `:1415` | asset import |
| GET `/experiment/models`, `/experiment/models/{folder}` | `:1432`, `:1448` | model library |
| GET `/view_metadata/{folder}?filename=` | `:1462` | safetensors metadata |
| GET `/system_stats` | `:1560` | system info / about page |
| POST `/queue` `{delete: [ids]}` or `{clear: true}` | `:1571-1600` | edit the queue |
| POST `/history` `{delete: [ids]}` or `{clear: true}` | `:1571-1600` | edit history |
| POST `/interrupt` `{prompt_id?}` | `:1605` | interrupt |
| POST `/jobs/{id}/cancel`; POST `/jobs/cancel {job_ids}` | `:1620`, `:1643` | cancel jobs |
| GET `/jobs?…`, `/jobs/{id}`, `/jobs/{id}/assets` | `fetchJobs.ts:62,177,218` | queue and history in 1.60 |
| GET/POST `/users` | `:1662`, `:1671` | multi-user |
| GET/POST `/settings`; GET/POST `/settings/{id}` | `:1685/:1715`, `:1708/:1729` | settings |
| GET `/userdata/{file}` | `:1740` | read a user file |
| POST `/userdata/{file}?overwrite=&full_info=` | `~:1767` | write a user file |
| DELETE `/userdata/{file}` | `:1810` | delete |
| POST `/userdata/{src}/move/{dest}?overwrite=` | `:1826` | rename |
| GET `/userdata?dir=&recurse=true&split=false&full_info=true` | `:1837` | list files |
| GET `/global_subgraphs`, `/global_subgraphs/{id}` | `:1850`, `:1869` | blueprint subgraphs |
| GET `/internal/logs`, `/internal/logs/raw`; PATCH/POST `/internal/logs/subscribe` | `:1878-1895` | terminal logs |
| GET `/internal/folder_paths` | `:1904` | folder paths |
| POST `/free {unload_models, free_memory}` | `:1926` | unload models |
| GET `/i18n` | `:1955` | custom-node translations |

### Endpoints called outside `api.ts`
- POST `/upload/image` (multipart `image`, `subfolder`, `type`, `overwrite`): `src/composables/node/useNodeImageUpload.ts:50`.
- POST `/upload/mask`: mask editor.
- GET `/view?filename=&subfolder=&type=` and `/viewvideo?`: `src/utils/resultItemUrl.ts:20,36`.
- GET `/node_replacements`: `nodeReplacementService.ts:6`.
- `/assets*` (cloud): `assetService.ts:190-193`.
- GET `/internal/files/input`: `assetsStore.ts:46`.